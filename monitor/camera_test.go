package main

import (
	"context"
	"net/http"
	"net/http/httptest"
	"strconv"
	"strings"
	"testing"
	"time"
)

// camServer는 플레이리스트를 흉내 냅니다. lastMod와 serverNow를 따로
// 두어, 스트림 서버의 시계가 이 서비스와 어긋난 상황을 만들 수 있습니다.
func camServer(t *testing.T, lastMod, serverNow time.Time, code int) *httptest.Server {
	t.Helper()
	return camServerSeq(t, lastMod, serverNow, code, nil)
}

// playlist는 주어진 시퀀스 번호를 단 플레이리스트를 만듭니다.
func playlist(seq int) string {
	return "#EXTM3U\n#EXT-X-VERSION:6\n#EXT-X-TARGETDURATION:1\n" +
		"#EXT-X-MEDIA-SEQUENCE:" + strconv.Itoa(seq) + "\n" +
		"#EXTINF:1.000000,\nseg00001.ts\n"
}

// camServerSeq는 요청마다 다음 응답을 내놓습니다. nil이면 본문이 없는
// 예전 방식(헤더만) 서버가 됩니다.
type camResp struct {
	seq     int
	lastMod time.Time
	now     time.Time
}

func camServerSeq(t *testing.T, lastMod, serverNow time.Time, code int, seq []camResp) *httptest.Server {
	t.Helper()
	var i int
	s := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		lm, now := lastMod, serverNow
		body := ""
		if seq != nil {
			c := seq[min(i, len(seq)-1)]
			i++
			lm, now, body = c.lastMod, c.now, playlist(c.seq)
		}
		if !lm.IsZero() {
			w.Header().Set("Last-Modified", lm.UTC().Format(http.TimeFormat))
		}
		if !now.IsZero() {
			w.Header().Set("Date", now.UTC().Format(http.TimeFormat))
		}
		w.WriteHeader(code)
		_, _ = w.Write([]byte(body))
	}))
	t.Cleanup(s.Close)
	return s
}

func probeOnce(t *testing.T, url string, stall time.Duration) cameraStatus {
	t.Helper()
	c := newCameraProbe(url, stall)
	c.probe(context.Background())
	return c.get()
}

func TestCamera갱신중이면live(t *testing.T) {
	now := time.Now()
	s := camServer(t, now.Add(-2*time.Second), now, http.StatusOK)

	if got := probeOnce(t, s.URL, 25*time.Second); got.Status != camLive {
		t.Fatalf("status = %q, 기대 live (%s)", got.Status, got.Error)
	}
}

// 이 테스트가 이 파일의 이유입니다. 서버는 200을 주고 세그먼트도
// 받아지지만 프레임은 몇 시간 전에 멈춘 상태입니다.
func TestCamera얼어붙으면stalled(t *testing.T) {
	now := time.Now()
	s := camServer(t, now.Add(-6*time.Hour), now, http.StatusOK)

	got := probeOnce(t, s.URL, 25*time.Second)
	if got.Status != camStalled {
		t.Fatalf("status = %q, 기대 stalled", got.Status)
	}
	if got.AgeS == nil || *got.AgeS < 6*3600-5 {
		t.Fatalf("age_s = %v, 6시간쯤이어야 합니다", got.AgeS)
	}
}

// 이 서비스는 클러스터에서, 스트림 서버는 맥에서 돕니다. 두 시계가
// 어긋나도 판정은 흔들리면 안 됩니다. 같은 응답 안의 Date와
// Last-Modified만 빼기 때문입니다.
func TestCamera시계가어긋나도흔들리지않는다(t *testing.T) {
	skewed := time.Now().Add(3 * time.Hour)
	s := camServer(t, skewed.Add(-2*time.Second), skewed, http.StatusOK)

	if got := probeOnce(t, s.URL, 25*time.Second); got.Status != camLive {
		t.Fatalf("status = %q, 기대 live — 시계 차이에 흔들렸습니다", got.Status)
	}
}

func TestCamera서버가꺼지면down(t *testing.T) {
	s := camServer(t, time.Now(), time.Now(), http.StatusOK)
	url := s.URL
	s.Close()

	if got := probeOnce(t, url, 25*time.Second); got.Status != camDown {
		t.Fatalf("status = %q, 기대 down", got.Status)
	}
}

func TestCamera404면down(t *testing.T) {
	s := camServer(t, time.Time{}, time.Now(), http.StatusNotFound)

	if got := probeOnce(t, s.URL, 25*time.Second); got.Status != camDown {
		t.Fatalf("status = %q, 기대 down", got.Status)
	}
}

// 신선도를 모르면 살아 있다고 하지 않습니다. "받아지니까 괜찮다"가
// 바로 지금 고치려는 착각입니다.
func TestCameraLastModified가없으면unknown(t *testing.T) {
	s := camServer(t, time.Time{}, time.Now(), http.StatusOK)

	if got := probeOnce(t, s.URL, 25*time.Second); got.Status != camUnknown {
		t.Fatalf("status = %q, 기대 unknown", got.Status)
	}
}

// 서버가 Date를 주지 않으면 이쪽 시계로 재는 수밖에 없습니다.
func TestCameraDate가없어도판정한다(t *testing.T) {
	s := camServer(t, time.Now().Add(-6*time.Hour), time.Time{}, http.StatusOK)

	if got := probeOnce(t, s.URL, 25*time.Second); got.Status != camStalled {
		t.Fatalf("status = %q, 기대 stalled", got.Status)
	}
}

func TestCamera첫관측은전이가아니다(t *testing.T) {
	s := camServer(t, time.Now(), time.Now(), http.StatusOK)
	c := newCameraProbe(s.URL, 25*time.Second)

	from, to := c.probe(context.Background())
	if from != camUnknown || to != camLive {
		t.Fatalf("첫 관측 = %q → %q, 기대 unknown → live", from, to)
	}
	// 같은 상태가 이어지면 전이가 아닙니다.
	if from, to := c.probe(context.Background()); from != "" || to != "" {
		t.Fatalf("같은 상태인데 전이로 봤습니다: %q → %q", from, to)
	}
}

// ── 알림 ───────────────────────────────────────────────────────────

func TestCamera경보는유예뒤에한번만(t *testing.T) {
	r, _, f := newTestRules(50)
	now := time.Now()

	r.checkCamera(camStalled, now)
	if len(f.sent) != 0 {
		t.Fatalf("유예 안에 보냈습니다: %v", f.sent)
	}

	r.checkCamera(camStalled, now.Add(4*time.Minute))
	if len(f.sent) != 0 {
		t.Fatalf("유예 안에 보냈습니다: %v", f.sent)
	}

	r.checkCamera(camStalled, now.Add(6*time.Minute))
	r.checkCamera(camStalled, now.Add(7*time.Minute))
	if len(f.sent) != 1 {
		t.Fatalf("보낸 횟수 = %d, 기대 1: %v", len(f.sent), f.sent)
	}
	if !contains(f.sent[0], "카메라가 멈췄습니다") {
		t.Fatalf("내용이 다릅니다: %s", f.sent[0])
	}
}

func TestCamera돌아오면해제(t *testing.T) {
	r, _, f := newTestRules(50)
	now := time.Now()

	r.checkCamera(camStalled, now)
	r.checkCamera(camStalled, now.Add(6*time.Minute))
	r.checkCamera(camLive, now.Add(7*time.Minute))

	if len(f.sent) != 2 {
		t.Fatalf("보낸 횟수 = %d, 기대 2: %v", len(f.sent), f.sent)
	}
	if !contains(f.sent[1], "카메라가 돌아왔습니다") {
		t.Fatalf("복구 알림이 아닙니다: %s", f.sent[1])
	}
}

// 아직 확인 전인 것을 고장으로 알리면, 서비스를 띄울 때마다 알림이
// 한 통씩 갑니다.
func TestCameraUnknown으로는알리지않는다(t *testing.T) {
	r, _, f := newTestRules(50)
	now := time.Now()

	r.checkCamera(camUnknown, now)
	r.checkCamera(camUnknown, now.Add(30*time.Minute))

	if len(f.sent) != 0 {
		t.Fatalf("unknown으로 알렸습니다: %v", f.sent)
	}
}

// ── 세그먼트 번호 ──────────────────────────────────────────────────

// 이 테스트가 두 번째 교훈입니다.
//
// ffmpeg은 종료할 때도 플레이리스트를 다시 씁니다. 그래서 감시
// 스크립트가 10초마다 재시작을 반복하는 동안 파일은 계속 방금 쓰인
// 것처럼 보입니다. 프레임은 한 장도 안 나오는데 말입니다.
func TestCamera파일만새것이고프레임은안나오면stalled(t *testing.T) {
	base := time.Now()
	var resps []camResp
	// 10초마다 재시작 — 파일은 매번 새로 쓰이지만 번호는 제자리.
	for i := 0; i < 6; i++ {
		at := base.Add(time.Duration(i) * 10 * time.Second)
		resps = append(resps, camResp{seq: 1541, lastMod: at, now: at})
	}
	s := camServerSeq(t, time.Time{}, time.Time{}, http.StatusOK, resps)

	c := newCameraProbe(s.URL, 25*time.Second)
	for i := 0; i < 5; i++ {
		c.probe(context.Background())
	}

	got := c.get()
	if got.Status != camStalled {
		t.Fatalf("status = %q, 기대 stalled — 파일이 새것이라고 속았습니다", got.Status)
	}
	if got.AgeS == nil || *got.AgeS < 20 {
		t.Fatalf("age_s = %v, 20초 넘게 나와야 합니다", got.AgeS)
	}
}

func TestCamera번호가늘면live(t *testing.T) {
	base := time.Now()
	var resps []camResp
	for i := 0; i < 6; i++ {
		at := base.Add(time.Duration(i) * 10 * time.Second)
		resps = append(resps, camResp{seq: 1541 + i*10, lastMod: at, now: at})
	}
	s := camServerSeq(t, time.Time{}, time.Time{}, http.StatusOK, resps)

	c := newCameraProbe(s.URL, 25*time.Second)
	for i := 0; i < 4; i++ {
		c.probe(context.Background())
	}

	if got := c.get(); got.Status != camLive {
		t.Fatalf("status = %q, 기대 live", got.Status)
	}
}

// 번호는 그대로인데 파일마저 늙으면, 둘 중 더 나쁜 쪽이 답입니다.
func TestCamera둘다늙으면더나쁜쪽을쓴다(t *testing.T) {
	now := time.Now()
	resps := []camResp{{seq: 1541, lastMod: now.Add(-6 * time.Hour), now: now}}
	s := camServerSeq(t, time.Time{}, time.Time{}, http.StatusOK, resps)

	got := probeOnce(t, s.URL, 25*time.Second)
	if got.Status != camStalled {
		t.Fatalf("status = %q, 기대 stalled", got.Status)
	}
	if got.AgeS == nil || *got.AgeS < 6*3600-5 {
		t.Fatalf("age_s = %v, 여섯 시간쯤이어야 합니다", got.AgeS)
	}
}

func Test미디어시퀀스를읽는다(t *testing.T) {
	seq, err := mediaSequence(strings.NewReader(playlist(1541)))
	if err != nil || seq != 1541 {
		t.Fatalf("seq = %d, err = %v", seq, err)
	}
	if _, err := mediaSequence(strings.NewReader("#EXTM3U\n")); err == nil {
		t.Fatal("번호가 없는데 읽었다고 합니다")
	}
}

// probeSeq는 주어진 번호들을 10초 간격으로 차례로 확인하고, 매번의
// 상태를 돌려줍니다.
func probeSeq(t *testing.T, seqs []int) []string {
	t.Helper()
	base := time.Now()
	var resps []camResp
	for i, n := range seqs {
		at := base.Add(time.Duration(i) * 10 * time.Second)
		resps = append(resps, camResp{seq: n, lastMod: at, now: at})
	}
	s := camServerSeq(t, time.Time{}, time.Time{}, http.StatusOK, resps)
	c := newCameraProbe(s.URL, 25*time.Second)

	var out []string
	for range seqs {
		c.probe(context.Background())
		out = append(out, c.get().Status)
	}
	return out
}

// 14시간 동안 실제로 벌어진 일입니다. 감시가 10초마다 ffmpeg을 죽이고,
// 죽을 때마다 쌓인 세그먼트가 한꺼번에 쓰여 번호가 가끔 한 번씩
// 뜁니다. 한 번 뛴 것을 복구로 보면 멈춤과 복구를 오가며 알림이
// 나갑니다.
func TestCamera한번뛴번호로는살아났다고하지않는다(t *testing.T) {
	// 정상 → 멈춤(제자리) → 한 번 뜀 → 다시 제자리 …
	seqs := []int{100, 110, 120, 120, 120, 120, 121, 121, 121, 122, 122, 122}
	got := probeSeq(t, seqs)

	stalledAt := -1
	for i, st := range got {
		if st == camStalled {
			stalledAt = i
			break
		}
	}
	if stalledAt < 0 {
		t.Fatalf("멈춤을 잡지 못했습니다: %v", got)
	}
	for i := stalledAt; i < len(got); i++ {
		if got[i] == camLive {
			t.Fatalf("%d번째 확인에서 live로 돌아갔습니다: %v", i, got)
		}
	}
}

func TestCamera연속으로늘면살아났다고한다(t *testing.T) {
	seqs := []int{100, 110, 120, 120, 120, 120, 125, 130, 135}
	got := probeSeq(t, seqs)

	if got[5] != camStalled {
		t.Fatalf("멈춤을 잡지 못했습니다: %v", got)
	}
	if got[6] != camStalled {
		t.Fatalf("한 번 늘었다고 바로 살렸습니다: %v", got)
	}
	if got[7] != camLive {
		t.Fatalf("두 번 연속 늘었는데 살리지 않았습니다: %v", got)
	}
}

// ffmpeg이 새로 뜨면 번호가 0부터 다시 시작합니다. 줄어든 것을 진행으로
// 세면 안 되지만, 그 뒤로 늘어나는 것은 진짜 진행입니다.
func TestCamera재시작으로줄어든번호는진행이아니다(t *testing.T) {
	seqs := []int{2160, 2170, 2180, 2180, 2180, 2180, 0, 0, 5, 10, 15}
	got := probeSeq(t, seqs)

	if got[5] != camStalled {
		t.Fatalf("멈춤을 잡지 못했습니다: %v", got)
	}
	if got[6] == camLive || got[7] == camLive {
		t.Fatalf("줄어든 번호를 복구로 봤습니다: %v", got)
	}
	if got[len(got)-1] != camLive {
		t.Fatalf("재시작 뒤 실제로 늘었는데 살리지 않았습니다: %v", got)
	}
}

// 복구를 보류하는 동안에도 나이는 멈춘 때부터 잽니다.
func TestCamera보류중에도나이는멈춘때부터(t *testing.T) {
	base := time.Now()
	seqs := []int{100, 110, 120, 120, 120, 120, 121}
	var resps []camResp
	for i, n := range seqs {
		at := base.Add(time.Duration(i) * 10 * time.Second)
		resps = append(resps, camResp{seq: n, lastMod: at, now: at})
	}
	s := camServerSeq(t, time.Time{}, time.Time{}, http.StatusOK, resps)
	c := newCameraProbe(s.URL, 25*time.Second)
	for range seqs {
		c.probe(context.Background())
	}

	got := c.get()
	if got.Status != camStalled {
		t.Fatalf("status = %q, 기대 stalled", got.Status)
	}
	if got.AgeS == nil || *got.AgeS < 30 {
		t.Fatalf("age_s = %v, 마지막 정상(20초 시점)부터 40초쯤이어야 합니다", got.AgeS)
	}
}
