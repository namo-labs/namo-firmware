package main

import "testing"

func sp(v string) *string { return &v }
func bp(v bool) *bool     { return &v }
func i64p(v int64) *int64 { return &v }

// 첫 관측에는 비교할 이전 값이 없습니다. 여기서 사건을 내면 서비스가
// 뜰 때마다 가짜 전이가 목록에 쌓입니다.
func Test첫관측은사건이없다(t *testing.T) {
	w := newWatcher()
	got := w.observe(stateView{
		TS: i64p(100), Reservoir: sp("ok"), Leak: sp("none"), Locked: bp(false),
	}, 100)
	if len(got) != 0 {
		t.Fatalf("사건 %d개, 기대 0개: %+v", len(got), got)
	}
}

func Test같은값은사건이없다(t *testing.T) {
	w := newWatcher()
	s := stateView{TS: i64p(100), Reservoir: sp("ok"), Leak: sp("none")}
	w.observe(s, 100)

	s.TS = i64p(110)
	if got := w.observe(s, 110); len(got) != 0 {
		t.Fatalf("사건 %d개, 기대 0개: %+v", len(got), got)
	}
}

func Test물통이비면사건(t *testing.T) {
	w := newWatcher()
	w.observe(stateView{TS: i64p(100), Reservoir: sp("ok")}, 100)
	got := w.observe(stateView{TS: i64p(160), Reservoir: sp("empty")}, 160)

	if len(got) != 1 {
		t.Fatalf("사건 %d개, 기대 1개", len(got))
	}
	e := got[0]
	if e.Kind != KindReservoir || e.From != "ok" || e.To != "empty" {
		t.Fatalf("사건이 예상과 다릅니다: %+v", e)
	}
	if e.TS != 160 {
		t.Fatalf("시각 = %d, 기대 160", e.TS)
	}
}

func Test누수와잠금이함께바뀌면둘다(t *testing.T) {
	w := newWatcher()
	w.observe(stateView{TS: i64p(100), Leak: sp("none"), Locked: bp(false)}, 100)
	got := w.observe(stateView{TS: i64p(160), Leak: sp("detected"), Locked: bp(true)}, 160)

	if len(got) != 2 {
		t.Fatalf("사건 %d개, 기대 2개: %+v", len(got), got)
	}
	kinds := map[string]bool{}
	for _, e := range got {
		kinds[e.Kind] = true
	}
	if !kinds[KindLeak] || !kinds[KindLock] {
		t.Fatalf("누수와 잠금이 모두 있어야 합니다: %+v", got)
	}
}

// nil에서 값으로 가는 것은 전이가 아니라 처음 알게 된 것입니다.
// 부팅 직후 게이트웨이 상태가 채워질 때 가짜 사건이 생기면 안 됩니다.
func Test미상에서값으로는사건이아니다(t *testing.T) {
	w := newWatcher()
	w.observe(stateView{TS: i64p(100), Gateway: nil}, 100)
	got := w.observe(stateView{TS: i64p(160), Gateway: bp(true)}, 160)

	if len(got) != 0 {
		t.Fatalf("사건 %d개, 기대 0개: %+v", len(got), got)
	}
}

func Test게이트웨이가죽으면사건(t *testing.T) {
	w := newWatcher()
	w.observe(stateView{TS: i64p(100), Gateway: bp(true)}, 100)
	got := w.observe(stateView{TS: i64p(160), Gateway: bp(false)}, 160)

	if len(got) != 1 || got[0].Kind != KindGateway || got[0].To != "false" {
		t.Fatalf("사건이 예상과 다릅니다: %+v", got)
	}
}

// 장치 시각이 아직 없으면 수신 시각으로 대신합니다.
func Test장치시각이없으면수신시각을쓴다(t *testing.T) {
	w := newWatcher()
	w.observe(stateView{Reservoir: sp("ok")}, 100)
	got := w.observe(stateView{Reservoir: sp("empty")}, 777)

	if len(got) != 1 || got[0].TS != 777 {
		t.Fatalf("시각이 예상과 다릅니다: %+v", got)
	}
}

func TestEventStore는새것부터돌려준다(t *testing.T) {
	s := NewEventStore(t.TempDir(), 86400)
	s.Add(Event{TS: 100, Kind: KindWater, Status: "completed"})
	s.Add(Event{TS: 200, Kind: KindWater, Status: "rejected", Reason: "cooldown"})

	got := s.Recent(10)
	if len(got) != 2 {
		t.Fatalf("길이 = %d, 기대 2", len(got))
	}
	if got[0].TS != 200 {
		t.Fatalf("첫 항목 = %d, 기대 200 (새것부터)", got[0].TS)
	}
}

func TestLastWater는조건에맞는최근것(t *testing.T) {
	s := NewEventStore(t.TempDir(), 86400)
	s.Add(Event{TS: 100, Kind: KindWater, Status: "completed", ML: 100})
	s.Add(Event{TS: 200, Kind: KindWater, Status: "rejected", Reason: "cooldown"})
	s.Add(Event{TS: 300, Kind: KindReservoir, From: "ok", To: "empty"})

	e, ok := s.LastWater("completed")
	if !ok || e.TS != 100 || e.ML != 100 {
		t.Fatalf("마지막 성공 급수가 예상과 다릅니다: %+v (ok=%v)", e, ok)
	}

	e, ok = s.LastWater("")
	if !ok || e.TS != 200 {
		t.Fatalf("마지막 급수 시도가 예상과 다릅니다: %+v", e)
	}
}

func TestLastTransition(t *testing.T) {
	s := NewEventStore(t.TempDir(), 86400)
	s.Add(Event{TS: 100, Kind: KindReservoir, From: "ok", To: "empty"})
	s.Add(Event{TS: 200, Kind: KindReservoir, From: "empty", To: "ok"})

	e, ok := s.LastTransition(KindReservoir, "empty")
	if !ok || e.TS != 100 {
		t.Fatalf("마지막 empty 전이가 예상과 다릅니다: %+v", e)
	}
}

func TestEventStore저장과재적재(t *testing.T) {
	dir := t.TempDir()
	s := NewEventStore(dir, 86400)
	s.Add(Event{TS: 100, Kind: KindWater, Status: "rejected", Reason: "cooldown"})
	if err := s.Flush(); err != nil {
		t.Fatalf("Flush 실패: %v", err)
	}

	again := NewEventStore(dir, 86400)
	got := again.Recent(10)
	if len(got) != 1 || got[0].Reason != "cooldown" {
		t.Fatalf("재적재가 예상과 다릅니다: %+v", got)
	}
}

func TestEventStore오래된것제거(t *testing.T) {
	s := NewEventStore(t.TempDir(), 1000)
	s.Add(Event{TS: 100, Kind: KindWater})
	s.Add(Event{TS: 200, Kind: KindWater})
	s.Add(Event{TS: 1500, Kind: KindWater}) // cutoff 500

	if s.Len() != 1 {
		t.Fatalf("길이 = %d, 기대 1", s.Len())
	}
}
