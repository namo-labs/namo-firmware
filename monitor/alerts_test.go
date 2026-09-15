package main

import (
	"testing"
	"time"
)

// 알림을 실제로 보내지 않고 무엇이 나갔는지만 봅니다.
type fakeSender struct{ sent []string }

func newTestRules(dryPct int) (*alertRules, *Notifier, *fakeSender) {
	f := &fakeSender{}
	n := &Notifier{
		token:  "test",
		chatID: "test",
		active: map[string]time.Time{},
		hook:   func(text string) { f.sent = append(f.sent, text) },
	}
	return newAlertRules(n, dryPct), n, f
}

func st(m int, reservoir, leak string, locked, gateway bool) alertState {
	return alertState{
		Moisture: &m, Reservoir: &reservoir, Leak: &leak,
		Locked: &locked, Gateway: &gateway,
	}
}

func TestDry경보는한번만(t *testing.T) {
	r, _, f := newTestRules(50)
	now := time.Now()

	r.check(st(45, "ok", "none", false, true), now)
	r.check(st(44, "ok", "none", false, true), now)
	r.check(st(43, "ok", "none", false, true), now)

	dry := 0
	for _, s := range f.sent {
		if contains(s, "흙이 말랐습니다") {
			dry++
		}
	}
	if dry != 1 {
		t.Fatalf("마름 알림 %d회, 기대 1회", dry)
	}
}

// 임계값 근처에서 오르내릴 때 알림이 반복되면 안 됩니다.
func TestDry경보는경계에서떨리지않는다(t *testing.T) {
	r, _, f := newTestRules(50)
	now := time.Now()

	r.check(st(49, "ok", "none", false, true), now) // 경보
	r.check(st(51, "ok", "none", false, true), now) // 살짝 회복 — 아직 안 내림
	r.check(st(49, "ok", "none", false, true), now) // 다시 내려감 — 이미 켜져 있음
	r.check(st(52, "ok", "none", false, true), now)

	for _, s := range f.sent {
		if contains(s, "흙이 젖었습니다") {
			t.Fatalf("margin 안에서 해제되면 안 됩니다: %q", s)
		}
	}
	if len(f.sent) != 1 {
		t.Fatalf("알림 %d회, 기대 1회: %v", len(f.sent), f.sent)
	}
}

func TestDry경보는충분히회복하면해제(t *testing.T) {
	r, _, f := newTestRules(50)
	now := time.Now()

	r.check(st(45, "ok", "none", false, true), now)
	r.check(st(62, "ok", "none", false, true), now) // 50 + 10 이상

	if len(f.sent) != 2 || !contains(f.sent[1], "흙이 젖었습니다") {
		t.Fatalf("해제 알림이 없습니다: %v", f.sent)
	}
}

func TestLeak경보와해제(t *testing.T) {
	r, _, f := newTestRules(50)
	now := time.Now()

	r.check(st(70, "ok", "detected", true, true), now)
	if len(f.sent) < 2 {
		t.Fatalf("누수와 잠금이 함께 알려져야 합니다: %v", f.sent)
	}
	if !contains(f.sent[0], "누수 감지") {
		t.Fatalf("첫 알림이 누수가 아닙니다: %q", f.sent[0])
	}

	before := len(f.sent)
	r.check(st(70, "ok", "none", true, true), now) // 물기는 없어졌지만 잠금은 유지
	if len(f.sent) != before+1 || !contains(f.sent[before], "누수가 해소") {
		t.Fatalf("누수 해제 알림이 없습니다: %v", f.sent)
	}
}

// 게이트웨이는 스스로 되살아나므로 잠깐 끊긴 것까지 알리지 않습니다.
func TestGateway는유예시간을둔다(t *testing.T) {
	r, _, f := newTestRules(50)
	base := time.Now()

	r.check(st(70, "ok", "none", false, false), base)
	if len(f.sent) != 0 {
		t.Fatalf("유예 안에 알리면 안 됩니다: %v", f.sent)
	}

	r.check(st(70, "ok", "none", false, false), base.Add(5*time.Minute))
	if len(f.sent) != 0 {
		t.Fatalf("아직 유예 안입니다: %v", f.sent)
	}

	r.check(st(70, "ok", "none", false, false), base.Add(11*time.Minute))
	if len(f.sent) != 1 || !contains(f.sent[0], "게이트웨이가 끊겼습니다") {
		t.Fatalf("유예를 넘겼으면 알려야 합니다: %v", f.sent)
	}
}

func TestGateway가돌아오면해제(t *testing.T) {
	r, _, f := newTestRules(50)
	base := time.Now()

	r.check(st(70, "ok", "none", false, false), base)
	r.check(st(70, "ok", "none", false, false), base.Add(11*time.Minute))
	r.check(st(70, "ok", "none", false, true), base.Add(12*time.Minute))

	if len(f.sent) != 2 || !contains(f.sent[1], "게이트웨이가 돌아왔습니다") {
		t.Fatalf("복구 알림이 없습니다: %v", f.sent)
	}
}

func TestReservoir경보와해제(t *testing.T) {
	r, _, f := newTestRules(50)
	now := time.Now()

	r.check(st(70, "empty", "none", false, true), now)
	r.check(st(70, "empty", "none", false, true), now) // 중복은 무시
	r.check(st(70, "ok", "none", false, true), now)

	if len(f.sent) != 2 {
		t.Fatalf("알림 %d회, 기대 2회(경보+해제): %v", len(f.sent), f.sent)
	}
}

// 텔레메트리가 끊기면 알리되, 유예 안에서는 조용합니다.
func TestDevice조용함(t *testing.T) {
	r, _, f := newTestRules(50)
	base := time.Now()

	r.checkQuiet(base, base.Add(2*time.Minute))
	if len(f.sent) != 0 {
		t.Fatalf("유예 안에 알리면 안 됩니다: %v", f.sent)
	}

	r.checkQuiet(base, base.Add(6*time.Minute))
	if len(f.sent) != 1 || !contains(f.sent[0], "응답하지 않습니다") {
		t.Fatalf("조용함 알림이 없습니다: %v", f.sent)
	}

	// 다시 값이 오면 해제됩니다.
	r.check(st(70, "ok", "none", false, true), base.Add(7*time.Minute))
	if len(f.sent) != 2 || !contains(f.sent[1], "장치가 돌아왔습니다") {
		t.Fatalf("복구 알림이 없습니다: %v", f.sent)
	}
}

// 설정이 없으면 조용히 넘어가야 합니다. 알림은 없어도 파일럿은 돌아야
// 합니다.
func TestNotifier없으면조용하다(t *testing.T) {
	r := newAlertRules(nil, 50)
	r.check(st(10, "empty", "detected", true, false), time.Now())
	r.checkQuiet(time.Now().Add(-time.Hour), time.Now())
	// 패닉 없이 지나가면 됩니다.
}

func TestNewNotifier는설정이없으면nil(t *testing.T) {
	if NewNotifier("", "123") != nil {
		t.Fatal("토큰이 없으면 nil이어야 합니다")
	}
	if NewNotifier("abc", "") != nil {
		t.Fatal("chat_id가 없으면 nil이어야 합니다")
	}
}

func contains(s, sub string) bool {
	return len(s) >= len(sub) && (func() bool {
		for i := 0; i+len(sub) <= len(s); i++ {
			if s[i:i+len(sub)] == sub {
				return true
			}
		}
		return false
	})()
}
