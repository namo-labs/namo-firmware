package main

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	mqtt "github.com/eclipse/paho.mqtt.golang"
)

func TestDeliver는기다리는쪽에전달(t *testing.T) {
	p := newPending()
	ch := p.add("abc")

	p.deliver("abc", json.RawMessage(`{"status":"completed"}`))

	select {
	case got := <-ch:
		if string(got) != `{"status":"completed"}` {
			t.Fatalf("받은 값 = %s", got)
		}
	case <-time.After(time.Second):
		t.Fatal("전달되지 않았습니다")
	}
}

// 기다리는 쪽이 없는 결과는 버려야 합니다. 장치가 스스로 보낸 급수나
// 다른 클라이언트의 명령 결과가 여기로 옵니다.
func TestDeliver는모르는id를무시(t *testing.T) {
	p := newPending()
	p.deliver("없는id", json.RawMessage(`{}`))
	// 패닉 없이 지나가면 됩니다.
}

// remove 뒤에 결과가 와도 막히면 안 됩니다. 타임아웃으로 빠져나간
// 요청의 결과가 뒤늦게 도착하는 경우입니다.
func TestDeliver는제거후에도막히지않는다(t *testing.T) {
	p := newPending()
	p.add("abc")
	p.remove("abc")

	done := make(chan struct{})
	go func() {
		p.deliver("abc", json.RawMessage(`{}`))
		close(done)
	}()

	select {
	case <-done:
	case <-time.After(time.Second):
		t.Fatal("deliver가 막혔습니다")
	}
}

// 같은 id로 결과가 두 번 와도 두 번째에서 막히면 안 됩니다.
func TestDeliver는중복에도막히지않는다(t *testing.T) {
	p := newPending()
	p.add("abc")

	p.deliver("abc", json.RawMessage(`{"n":1}`))

	done := make(chan struct{})
	go func() {
		p.deliver("abc", json.RawMessage(`{"n":2}`))
		close(done)
	}()

	select {
	case <-done:
	case <-time.After(time.Second):
		t.Fatal("두 번째 deliver가 막혔습니다")
	}
}

func TestNewID는매번다르다(t *testing.T) {
	seen := make(map[string]bool)
	for i := 0; i < 100; i++ {
		id := newID()
		if seen[id] {
			t.Fatalf("중복 id: %s", id)
		}
		seen[id] = true
	}
}

func TestSameOrigin(t *testing.T) {
	cases := []struct {
		origin, host string
		want         bool
	}{
		{"https://kang1027.com", "kang1027.com", true},
		{"http://kang1027.com", "kang1027.com", true},
		// 리버스 프록시 뒤라 바깥 포트와 안쪽 호스트가 다를 수 있습니다.
		{"https://kang1027.com:443", "kang1027.com", true},
		{"http://localhost:8099", "localhost:8099", true},
		{"https://evil.com", "kang1027.com", false},
		{"https://kang1027.com.evil.com", "kang1027.com", false},
	}
	for _, c := range cases {
		if got := sameOrigin(c.origin, c.host); got != c.want {
			t.Errorf("sameOrigin(%q, %q) = %v, 기대 %v", c.origin, c.host, got, c.want)
		}
	}
}

// 장치의 1회 상한을 넘는 값은 애초에 보내지 않습니다. 장치가 거부하겠지만
// 웹에서 걸러 두면 헛된 왕복이 줄고, 실수로 큰 값을 넣는 일도 막습니다.
func TestAllowedDoses는상한안에있다(t *testing.T) {
	const deviceMax = 300
	for dose := range allowedDoses {
		if dose <= 0 || dose > deviceMax {
			t.Errorf("허용 급수량 %dmL가 장치 상한(%d)을 벗어납니다", dose, deviceMax)
		}
	}
}

// Origin도 Sec-Fetch-Site도 없으면 거부해야 합니다. 판단할 근거가 없을 때
// 허용하면, 헤더를 가리는 것만으로 검사를 지나칠 수 있습니다.
func TestWater는출처를모르면거부(t *testing.T) {
	rec := postWater(t, map[string]string{"Content-Type": "application/json"}, `{"dose_ml":50}`)
	if rec.Code != http.StatusForbidden {
		t.Fatalf("코드 = %d, 기대 403", rec.Code)
	}
}

// HTML form은 application/json을 만들 수 없습니다. Content-Type을 강제하면
// 남의 페이지에 숨긴 폼으로 급수를 거는 길이 막힙니다.
func TestWater는form전송을거부(t *testing.T) {
	rec := postWater(t, map[string]string{
		"Content-Type": "text/plain",
		"Origin":       "http://example.test",
	}, `{"dose_ml":50}`)
	if rec.Code != http.StatusUnsupportedMediaType {
		t.Fatalf("코드 = %d, 기대 415", rec.Code)
	}
}

func TestWater는다른출처를거부(t *testing.T) {
	rec := postWater(t, map[string]string{
		"Content-Type": "application/json",
		"Origin":       "https://evil.com",
	}, `{"dose_ml":50}`)
	if rec.Code != http.StatusForbidden {
		t.Fatalf("코드 = %d, 기대 403", rec.Code)
	}
}

func TestWater는GET을거부(t *testing.T) {
	h := waterHandler(nil, "pilot01", newPending(), NewEventStore(t.TempDir(), 86400))
	req := httptest.NewRequest(http.MethodGet, "http://example.test/api/water", nil)
	rec := httptest.NewRecorder()
	h(rec, req)
	if rec.Code != http.StatusMethodNotAllowed {
		t.Fatalf("코드 = %d, 기대 405", rec.Code)
	}
}

// 여기까지 통과한 뒤에야 급수량을 봅니다. 장치 상한을 넘는 값은 보내기
// 전에 걸러 헛된 왕복을 줄입니다.
func TestWater는허용되지않은급수량을거부(t *testing.T) {
	rec := postWater(t, map[string]string{
		"Content-Type":   "application/json",
		"Sec-Fetch-Site": "same-origin",
	}, `{"dose_ml":999}`)
	if rec.Code != http.StatusBadRequest {
		t.Fatalf("코드 = %d, 기대 400", rec.Code)
	}
}

func postWater(t *testing.T, headers map[string]string, body string) *httptest.ResponseRecorder {
	t.Helper()
	h := waterHandler(nil, "pilot01", newPending(), NewEventStore(t.TempDir(), 86400))
	req := httptest.NewRequest(http.MethodPost, "http://example.test/api/water", strings.NewReader(body))
	for k, v := range headers {
		req.Header.Set(k, v)
	}
	rec := httptest.NewRecorder()
	h(rec, req)
	return rec
}

// fakeClient는 paho 클라이언트의 연결 상태와 발행만 흉내 냅니다.
//
// IsConnected는 재접속 중에도 true를 줍니다(AutoReconnect일 때). 브로커가
// 죽은 동안의 실제 동작이 이것이라, 테스트도 같게 둡니다.
type fakeClient struct {
	mqtt.Client // 쓰지 않는 메서드는 nil이라 부르면 바로 패닉합니다.
	open        bool
	token       mqtt.Token
	published   int
}

func (f *fakeClient) IsConnected() bool      { return true }
func (f *fakeClient) IsConnectionOpen() bool { return f.open }
func (f *fakeClient) Publish(string, byte, bool, interface{}) mqtt.Token {
	f.published++
	return f.token
}

// hangToken은 브로커가 응답하지 않을 때의 발행 토큰입니다. 재접속 전까지
// 끝나지 않습니다.
type hangToken struct{}

func (hangToken) Wait() bool                       { select {} }
func (hangToken) WaitTimeout(d time.Duration) bool { time.Sleep(d); return false }
func (hangToken) Done() <-chan struct{}            { return nil }
func (hangToken) Error() error                     { return nil }

func serveWithin(t *testing.T, h func(http.ResponseWriter, *http.Request), req *http.Request, limit time.Duration) *httptest.ResponseRecorder {
	t.Helper()
	rec := httptest.NewRecorder()
	done := make(chan struct{})
	go func() {
		h(rec, req)
		close(done)
	}()
	select {
	case <-done:
		return rec
	case <-time.After(limit):
		t.Fatalf("핸들러가 %v 안에 끝나지 않았습니다 (웹이 \"요청 중…\"에서 멈춥니다)", limit)
		return nil
	}
}

func postJSON(path, body string) *http.Request {
	req := httptest.NewRequest(http.MethodPost, "http://example.test"+path, strings.NewReader(body))
	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("Sec-Fetch-Site", "same-origin")
	return req
}

// 브로커가 죽어 paho가 재접속 중이면 IsConnected는 true지만 연결은 열려 있지
// 않습니다. 이때 발행하면 명령이 큐에만 쌓이고 결과를 영영 못 받습니다.
func TestWater는재접속중이면발행하지않고거부(t *testing.T) {
	c := &fakeClient{open: false, token: hangToken{}}
	h := waterHandler(c, "pilot01", newPending(), NewEventStore(t.TempDir(), 86400))

	rec := serveWithin(t, h, postJSON("/api/water", `{"dose_ml":100}`), 2*time.Second)

	if rec.Code != http.StatusServiceUnavailable {
		t.Fatalf("코드 = %d, 기대 503", rec.Code)
	}
	if c.published != 0 {
		t.Fatalf("연결이 열려 있지 않은데 %d번 발행했습니다", c.published)
	}
}

// 연결은 열려 있는데 브로커가 발행을 확인해 주지 않아도 무한정 기다리지
// 않아야 합니다.
func TestWater는발행확인이없으면제한시간에끝난다(t *testing.T) {
	old := publishTimeout
	publishTimeout = 50 * time.Millisecond
	t.Cleanup(func() { publishTimeout = old })

	c := &fakeClient{open: true, token: hangToken{}}
	h := waterHandler(c, "pilot01", newPending(), NewEventStore(t.TempDir(), 86400))

	rec := serveWithin(t, h, postJSON("/api/water", `{"dose_ml":100}`), 2*time.Second)

	if rec.Code != http.StatusGatewayTimeout {
		t.Fatalf("코드 = %d, 기대 504", rec.Code)
	}
}

func TestUnlock은재접속중이면발행하지않고거부(t *testing.T) {
	c := &fakeClient{open: false, token: hangToken{}}
	h := unlockHandler(c, "pilot01", &state{})

	rec := serveWithin(t, h, postJSON("/api/unlock", `{}`), 2*time.Second)

	if rec.Code != http.StatusServiceUnavailable {
		t.Fatalf("코드 = %d, 기대 503", rec.Code)
	}
	if c.published != 0 {
		t.Fatalf("연결이 열려 있지 않은데 %d번 발행했습니다", c.published)
	}
}

func TestUnlock은발행확인이없으면제한시간에끝난다(t *testing.T) {
	old := publishTimeout
	publishTimeout = 50 * time.Millisecond
	t.Cleanup(func() { publishTimeout = old })

	c := &fakeClient{open: true, token: hangToken{}}
	h := unlockHandler(c, "pilot01", &state{})

	rec := serveWithin(t, h, postJSON("/api/unlock", `{}`), 2*time.Second)

	if rec.Code != http.StatusGatewayTimeout {
		t.Fatalf("코드 = %d, 기대 504", rec.Code)
	}
}
