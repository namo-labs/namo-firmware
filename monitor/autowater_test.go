package main

import (
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"
)

var t0 = time.Date(2026, 10, 18, 12, 0, 0, 0, time.Local)

func newTestAuto(t *testing.T, on bool) (*autoWaterer, *fakeSender) {
	t.Helper()
	f := &fakeSender{}
	n := &Notifier{
		token:  "test",
		chatID: "test",
		active: map[string]time.Time{},
		hook:   func(text string) { f.sent = append(f.sent, text) },
	}
	return newAutoWaterer(n, t.TempDir(), on, 50, 100), f
}

// okView는 물을 주기에 모든 조건이 맞는 텔레메트리입니다. 테스트마다
// 한 가지만 바꿔 그 조건이 하는 일을 봅니다.
func okView(m int) autoView {
	ts := int64(1772500000)
	ago := 20
	reservoir, leak, pump := "ok", "none", "idle"
	locked := false
	today := 0
	return autoView{
		TS: &ts, Moisture: &m, SensorAgo: &ago,
		Reservoir: &reservoir, Leak: &leak, Locked: &locked,
		Pump: &pump, Today: &today,
	}
}

func withToday(v autoView, ml int) autoView         { v.Today = &ml; return v }
func withReservoir(v autoView, s string) autoView   { v.Reservoir = &s; return v }
func withLeak(v autoView, s string) autoView        { v.Leak = &s; return v }
func withLocked(v autoView, b bool) autoView        { v.Locked = &b; return v }
func withPump(v autoView, s string) autoView        { v.Pump = &s; return v }
func withSensorAgo(v autoView, s int) autoView      { v.SensorAgo = &s; return v }
func withCooldown(v autoView, until int64) autoView { v.CooldownUntil = &until; return v }

func completedSend(dose int) (waterResult, error) {
	return waterResult{Status: "completed", EstimatedML: dose}, nil
}

func countWith(sent []string, sub string) int {
	n := 0
	for _, s := range sent {
		if strings.Contains(s, sub) {
			n++
		}
	}
	return n
}

// waterOnce는 t0에 한 번 줘서 완료된 상태를 만듭니다.
func waterOnce(t *testing.T, a *autoWaterer, m int) {
	t.Helper()
	p, ok := a.decide(okView(m), t0)
	if !ok {
		t.Fatal("첫 급수가 나가지 않았습니다")
	}
	a.execute(p, completedSend)
}

func TestAuto는마르면정해진양을준다(t *testing.T) {
	a, _ := newTestAuto(t, true)

	p, ok := a.decide(okView(44), t0)

	if !ok || p.dose != 100 {
		t.Fatalf("plan = %+v, ok = %v; 기대 100mL", p, ok)
	}
}

func TestAuto는꺼져있으면주지않는다(t *testing.T) {
	a, f := newTestAuto(t, false)

	if _, ok := a.decide(okView(30), t0); ok {
		t.Fatal("꺼져 있는데 물을 줬습니다")
	}
	if len(f.sent) != 0 {
		t.Fatalf("꺼져 있는데 알렸습니다: %v", f.sent)
	}
}

func TestAuto는기준아래에서만준다(t *testing.T) {
	cases := []struct {
		m    int
		want bool
	}{{50, false}, {49, true}}
	for _, c := range cases {
		a, _ := newTestAuto(t, true)
		if _, ok := a.decide(okView(c.m), t0); ok != c.want {
			t.Errorf("흙수분 %d%%: 급수 = %v, 기대 %v", c.m, ok, c.want)
		}
	}
}

// 모르면 주지 않습니다. 센서값이 오래됐으면 지금 흙이 젖었는지 알 수 없습니다.
func TestAuto는센서값을모르면주지않는다(t *testing.T) {
	noMoisture := okView(44)
	noMoisture.Moisture = nil
	noAgo := okView(44)
	noAgo.SensorAgo = nil

	cases := []struct {
		name string
		v    autoView
		want bool
	}{
		{"흙수분 없음", noMoisture, false},
		{"수신 경과 없음", noAgo, false},
		{"301초 전", withSensorAgo(okView(44), 301), false},
		{"300초 전", withSensorAgo(okView(44), 300), true},
	}
	for _, c := range cases {
		a, _ := newTestAuto(t, true)
		if _, ok := a.decide(c.v, t0); ok != c.want {
			t.Errorf("%s: 급수 = %v, 기대 %v", c.name, ok, c.want)
		}
	}
}

// 쿨다운·급수 중·시각 미동기는 곧 풀립니다. 알리지 않고 기다립니다.
func TestAuto는장치가쉴때까지조용히기다린다(t *testing.T) {
	noTS := okView(44)
	noTS.TS = nil

	cases := []struct {
		name string
		v    autoView
	}{
		{"쿨다운", withCooldown(okView(44), 1772501800)},
		{"급수 중", withPump(okView(44), "running")},
		{"시각 모름", noTS},
	}
	for _, c := range cases {
		a, f := newTestAuto(t, true)
		if _, ok := a.decide(c.v, t0); ok {
			t.Errorf("%s: 물을 줬습니다", c.name)
		}
		if len(f.sent) != 0 {
			t.Errorf("%s: 알렸습니다: %v", c.name, f.sent)
		}
	}
}

// 사람이 손을 써야 풀리는 이유로 못 주면 알립니다. 흙은 마르는데 아무 말이
// 없으면 화분이 그대로 말라 죽습니다.
func TestAuto는못주는이유를알린다(t *testing.T) {
	cases := []struct {
		name string
		v    autoView
		want string
	}{
		{"물통 빔", withReservoir(okView(44), "empty"), "물통이 비었습니다"},
		{"물통 모름", withReservoir(okView(44), "unknown"), "물통 상태를 모릅니다"},
		{"누수", withLeak(okView(44), "detected"), "누수가 감지됐습니다"},
		{"누수 모름", withLeak(okView(44), "unknown"), "누수 상태를 믿을 수 없습니다"},
		{"잠김", withLocked(okView(44), true), "누수로 잠겨 있습니다"},
		{"한도", withToday(okView(44), 450), "오늘 한도를 다 썼습니다"},
	}
	for _, c := range cases {
		a, f := newTestAuto(t, true)
		if _, ok := a.decide(c.v, t0); ok {
			t.Errorf("%s: 물을 줬습니다", c.name)
		}
		if len(f.sent) != 1 || !strings.Contains(f.sent[0], c.want) || !strings.Contains(f.sent[0], "44%") {
			t.Errorf("%s: 알림 = %v, 기대 %q와 흙수분", c.name, f.sent, c.want)
		}
	}
}

// 한도에 딱 맞으면 줍니다. 400 + 100 = 500mL.
func TestAuto는한도에딱맞으면준다(t *testing.T) {
	a, _ := newTestAuto(t, true)
	if _, ok := a.decide(withToday(okView(44), 400), t0); !ok {
		t.Fatal("한도 안인데 주지 않았습니다")
	}
}

func TestAuto는못주는이유를한번만알린다(t *testing.T) {
	a, f := newTestAuto(t, true)
	empty := withReservoir(okView(44), "empty")

	a.decide(empty, t0)
	a.decide(empty, t0.Add(time.Minute))
	a.decide(empty, t0.Add(2*time.Hour))

	if len(f.sent) != 1 {
		t.Fatalf("알림 %d회, 기대 1회: %v", len(f.sent), f.sent)
	}
}

func TestAuto는준뒤에알린다(t *testing.T) {
	a, f := newTestAuto(t, true)

	p, _ := a.decide(withToday(okView(44), 300), t0)
	a.execute(p, completedSend)

	if len(f.sent) != 1 {
		t.Fatalf("알림 %d회, 기대 1회: %v", len(f.sent), f.sent)
	}
	for _, want := range []string{"100mL", "44%", "400mL / 500mL"} {
		if !strings.Contains(f.sent[0], want) {
			t.Errorf("알림에 %q가 없습니다: %q", want, f.sent[0])
		}
	}
}

// 30분 안에는 다시 보내지 않습니다. 장치가 재부팅해 쿨다운을 잊어도
// 연달아 주면 안 됩니다. 30분이 지나도 마른 채면 다시 줍니다.
func TestAuto는30분뒤에도마르면다시준다(t *testing.T) {
	a, _ := newTestAuto(t, true)
	waterOnce(t, a, 44)
	a.decide(okView(48), t0.Add(5*time.Minute)) // 올랐으니 효과는 있었습니다

	if _, ok := a.decide(okView(47), t0.Add(29*time.Minute)); ok {
		t.Fatal("30분 안에 다시 줬습니다")
	}
	if _, ok := a.decide(okView(47), t0.Add(31*time.Minute)); !ok {
		t.Fatal("30분이 지나도 마른 채인데 주지 않았습니다")
	}
}

// completed는 GPIO까지만 보증합니다. 모터 선이 빠지면 completed인데 물이
// 나가지 않습니다(2026-10-13). 흙수분이 오르지 않으면 멈추고 알립니다.
func TestAuto는줬는데흙수분이오르지않으면멈춘다(t *testing.T) {
	a, f := newTestAuto(t, true)
	waterOnce(t, a, 44)

	a.decide(okView(44), t0.Add(5*time.Minute))
	a.decide(okView(46), t0.Add(16*time.Minute)) // +2%p는 오른 것으로 치지 않습니다
	a.decide(okView(45), t0.Add(20*time.Minute))

	if n := countWith(f.sent, "자동급수를 멈췄습니다"); n != 1 {
		t.Fatalf("멈춤 알림 %d회, 기대 1회: %v", n, f.sent)
	}
	if _, ok := a.decide(okView(44), t0.Add(31*time.Minute)); ok {
		t.Fatal("멈췄는데 다시 줬습니다")
	}
}

// 확인 창 안에 센서값이 끊기면 효과를 확인하지 못한 것입니다. 오래된
// 값은 올랐어도 믿지 않습니다.
func TestAuto는효과를확인하지못해도멈춘다(t *testing.T) {
	a, f := newTestAuto(t, true)
	waterOnce(t, a, 44)

	a.decide(withSensorAgo(okView(60), 900), t0.Add(16*time.Minute))

	if n := countWith(f.sent, "자동급수를 멈췄습니다"); n != 1 {
		t.Fatalf("멈춤 알림 %d회, 기대 1회: %v", n, f.sent)
	}
}

// 사람이 손으로 물을 줘 흙이 충분히 젖으면 다시 맡깁니다.
func TestAuto는멈춘뒤흙이젖으면재개한다(t *testing.T) {
	a, f := newTestAuto(t, true)
	waterOnce(t, a, 44)
	a.decide(okView(44), t0.Add(16*time.Minute)) // 멈춤

	a.decide(okView(60), t0.Add(time.Hour))
	if countWith(f.sent, "다시 켭니다") != 1 {
		t.Fatalf("재개 알림이 없습니다: %v", f.sent)
	}
	if _, ok := a.decide(okView(44), t0.Add(2*time.Hour)); !ok {
		t.Fatal("재개했는데 주지 않았습니다")
	}
}

// 배선을 고친 뒤 화면에서 다시 켜면 흙이 젖기를 기다리지 않고 재개합니다.
func TestAuto는다시켜면멈춤이풀린다(t *testing.T) {
	a, _ := newTestAuto(t, true)
	waterOnce(t, a, 44)
	a.decide(okView(44), t0.Add(16*time.Minute)) // 멈춤

	if err := a.SetEnabled(true); err != nil {
		t.Fatal(err)
	}
	if _, ok := a.decide(okView(44), t0.Add(31*time.Minute)); !ok {
		t.Fatal("다시 켰는데 주지 않았습니다")
	}
}

// 장치가 거부하면 한 번만 알리고, 30분 뒤에 다시 시도합니다. 같은 이유로
// 계속 거부돼도 다시 알리지 않습니다.
func TestAuto는거부되면한번만알린다(t *testing.T) {
	a, f := newTestAuto(t, true)
	reject := func(int) (waterResult, error) {
		return waterResult{Status: "rejected", Reason: "clock_unsynced"}, nil
	}

	p, _ := a.decide(okView(44), t0)
	a.execute(p, reject)
	if _, ok := a.decide(okView(44), t0.Add(10*time.Minute)); ok {
		t.Fatal("거부된 지 10분 만에 다시 보냈습니다")
	}
	p, ok := a.decide(okView(44), t0.Add(31*time.Minute))
	if !ok {
		t.Fatal("30분 뒤에 다시 시도하지 않았습니다")
	}
	a.execute(p, reject)

	if n := countWith(f.sent, "장치 시각이 맞춰지지 않았습니다"); n != 1 {
		t.Fatalf("거부 알림 %d회, 기대 1회: %v", n, f.sent)
	}
	// 거부는 물이 나가지 않은 것이라 효과 확인 대상이 아닙니다.
	if n := countWith(f.sent, "자동급수를 멈췄습니다"); n != 0 {
		t.Fatalf("거부를 효과 없음으로 봤습니다: %v", f.sent)
	}
}

// 결과를 못 받으면 물이 나갔는지 모릅니다. 보낸 것으로 쳐서 30분 안에
// 다시 보내지 않습니다.
func TestAuto는결과를모르면보낸것으로친다(t *testing.T) {
	a, f := newTestAuto(t, true)
	lost := func(int) (waterResult, error) { return waterResult{}, errNoResult }

	p, _ := a.decide(okView(44), t0)
	a.execute(p, lost)

	if countWith(f.sent, "결과를 받지 못했습니다") != 1 {
		t.Fatalf("알림이 없습니다: %v", f.sent)
	}
	if _, ok := a.decide(okView(44), t0.Add(10*time.Minute)); ok {
		t.Fatal("결과를 모르는데 10분 만에 다시 보냈습니다")
	}
}

func TestAuto는켜짐을파일에남긴다(t *testing.T) {
	dir := t.TempDir()

	a := newAutoWaterer(nil, dir, false, 50, 100)
	if err := a.SetEnabled(true); err != nil {
		t.Fatal(err)
	}
	if !newAutoWaterer(nil, dir, false, 50, 100).Enabled() {
		t.Fatal("다시 띄우니 꺼졌습니다")
	}

	if err := a.SetEnabled(false); err != nil {
		t.Fatal(err)
	}
	// 화면에서 끈 것은 처음 값(AUTO_WATER)보다 앞섭니다. 재시작했다고
	// 다시 켜져 물이 나가면 안 됩니다.
	if newAutoWaterer(nil, dir, true, 50, 100).Enabled() {
		t.Fatal("끈 것이 재시작하자 켜졌습니다")
	}
}

func TestAuto는저장하지못하면바꾸지않는다(t *testing.T) {
	a := newAutoWaterer(nil, "/없는/경로", false, 50, 100)

	if err := a.SetEnabled(true); err == nil {
		t.Fatal("저장하지 못했는데 오류가 없습니다")
	}
	if a.Enabled() {
		t.Fatal("저장하지 못했는데 켜졌습니다")
	}
}

func TestAutoAPI는켜고끈다(t *testing.T) {
	a := newAutoWaterer(nil, t.TempDir(), false, 50, 100)
	h := autoHandler(a)

	rec := httptest.NewRecorder()
	h(rec, postJSON("/api/auto", `{"enabled":true}`))
	if rec.Code != http.StatusOK || !a.Enabled() {
		t.Fatalf("코드 = %d, 켜짐 = %v", rec.Code, a.Enabled())
	}
	if !strings.Contains(rec.Body.String(), `"enabled":true`) {
		t.Fatalf("응답 = %s", rec.Body.String())
	}

	rec = httptest.NewRecorder()
	h(rec, httptest.NewRequest(http.MethodGet, "http://example.test/api/auto", nil))
	if rec.Code != http.StatusOK || !strings.Contains(rec.Body.String(), `"dose_ml":100`) {
		t.Fatalf("GET 코드 = %d, 응답 = %s", rec.Code, rec.Body.String())
	}
}

// 켜는 것은 물을 나가게 하는 일이라 급수와 같은 방어를 겁니다.
func TestAutoAPI는다른출처를거부(t *testing.T) {
	a := newAutoWaterer(nil, t.TempDir(), false, 50, 100)
	req := httptest.NewRequest(http.MethodPost, "http://example.test/api/auto", strings.NewReader(`{"enabled":true}`))
	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("Origin", "https://evil.com")
	rec := httptest.NewRecorder()

	autoHandler(a)(rec, req)

	if rec.Code != http.StatusForbidden || a.Enabled() {
		t.Fatalf("코드 = %d, 켜짐 = %v; 기대 403, 꺼짐", rec.Code, a.Enabled())
	}
}

func TestAutoAPI는enabled가없으면거부(t *testing.T) {
	a := newAutoWaterer(nil, t.TempDir(), true, 50, 100)
	rec := httptest.NewRecorder()

	autoHandler(a)(rec, postJSON("/api/auto", `{}`))

	if rec.Code != http.StatusBadRequest || !a.Enabled() {
		t.Fatalf("코드 = %d, 켜짐 = %v; 기대 400, 그대로", rec.Code, a.Enabled())
	}
}

// 자동급수가 켜져 있으면 "흙이 말랐습니다"는 보내지 않습니다. 급수 알림과
// 같은 일을 두 번 알리게 됩니다.
func TestDry경보는자동급수가켜져있으면보내지않는다(t *testing.T) {
	r, _, f := newTestRules(50)
	r.autoOn = func() bool { return true }

	r.check(st(45, "ok", "none", false, true), time.Now())

	if countWith(f.sent, "흙이 말랐습니다") != 0 {
		t.Fatalf("자동급수가 켜져 있는데 마름 알림이 나갔습니다: %v", f.sent)
	}
}

// 브로커가 끊겨 명령이 아예 나가지 않았으면 "물이 나갔는지 모른다"고
// 하면 안 됩니다. 사람이 펌프를 확인하러 갈 이유가 없습니다.
func TestAuto는보내지못한것과결과를모르는것을구분한다(t *testing.T) {
	a, f := newTestAuto(t, true)
	down := func(int) (waterResult, error) { return waterResult{}, errBrokerDown }

	p, _ := a.decide(okView(44), t0)
	a.execute(p, down)

	if len(f.sent) != 1 || !strings.Contains(f.sent[0], "보내지 못했습니다") || strings.Contains(f.sent[0], "나갔는지") {
		t.Fatalf("알림 = %v", f.sent)
	}
}
