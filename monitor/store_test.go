package main

import (
	"testing"
)

func intp(v int) *int { return &v }

func TestAddSensor솎아내기(t *testing.T) {
	s := NewStore(t.TempDir(), 60, 86400)

	s.AddSensor(Sample{TS: 1000, Moisture: intp(50)})
	s.AddSensor(Sample{TS: 1010, Moisture: intp(51)}) // 10초 뒤 — 버려집니다
	s.AddSensor(Sample{TS: 1060, Moisture: intp(52)}) // 60초 뒤 — 담깁니다

	if got := s.Len(); got != 2 {
		t.Fatalf("표본 수 = %d, 기대 2", got)
	}
}

// 급수는 드물고 중요하므로 간격 제한을 받지 않아야 합니다.
func TestAddWatering은간격무시(t *testing.T) {
	s := NewStore(t.TempDir(), 60, 86400)

	s.AddSensor(Sample{TS: 1000, Moisture: intp(50)})
	s.AddWatering(1005, 100)
	s.AddWatering(1006, 200)

	if got := s.Len(); got != 3 {
		t.Fatalf("표본 수 = %d, 기대 3", got)
	}
}

func TestSince(t *testing.T) {
	s := NewStore(t.TempDir(), 0, 86400)
	for ts := int64(100); ts <= 500; ts += 100 {
		s.AddSensor(Sample{TS: ts, Moisture: intp(int(ts))})
	}

	got := s.Since(300)
	if len(got) != 3 {
		t.Fatalf("길이 = %d, 기대 3", len(got))
	}
	if got[0].TS != 300 {
		t.Fatalf("첫 표본 = %d, 기대 300", got[0].TS)
	}
}

// Since가 돌려준 슬라이스를 호출부가 고쳐도 내부 상태가 바뀌면 안 됩니다.
func TestSince는복사본을준다(t *testing.T) {
	s := NewStore(t.TempDir(), 0, 86400)
	s.AddSensor(Sample{TS: 100, Moisture: intp(50)})

	got := s.Since(0)
	got[0].TS = 999

	if again := s.Since(0); again[0].TS != 100 {
		t.Fatalf("내부 표본이 바뀌었습니다: %d", again[0].TS)
	}
}

func TestTrim오래된것제거(t *testing.T) {
	s := NewStore(t.TempDir(), 0, 1000)

	s.AddSensor(Sample{TS: 100, Moisture: intp(1)})
	s.AddSensor(Sample{TS: 200, Moisture: intp(2)})
	// 1500 기준으로 보면 cutoff가 500이라 앞의 둘은 빠집니다.
	s.AddSensor(Sample{TS: 1500, Moisture: intp(3)})

	if got := s.Len(); got != 1 {
		t.Fatalf("표본 수 = %d, 기대 1", got)
	}
}

func TestFlush와재적재(t *testing.T) {
	dir := t.TempDir()

	s := NewStore(dir, 0, 86400)
	s.AddSensor(Sample{TS: 100, Moisture: intp(42)})
	s.AddWatering(150, 100)
	if err := s.Flush(); err != nil {
		t.Fatalf("Flush 실패: %v", err)
	}

	reloaded := NewStore(dir, 0, 86400)
	got := reloaded.Since(0)
	if len(got) != 2 {
		t.Fatalf("재적재 길이 = %d, 기대 2", len(got))
	}
	if got[0].Moisture == nil || *got[0].Moisture != 42 {
		t.Fatalf("흙수분이 살아남지 못했습니다: %+v", got[0])
	}
	if got[1].WaterML != 100 {
		t.Fatalf("급수량 = %d, 기대 100", got[1].WaterML)
	}
}

// 바뀐 것이 없으면 파일을 건드리지 않아야 합니다.
func TestFlush는변경없으면쓰지않는다(t *testing.T) {
	dir := t.TempDir()
	s := NewStore(dir, 0, 86400)

	if err := s.Flush(); err != nil {
		t.Fatalf("Flush 실패: %v", err)
	}
	// 표본이 없으면 파일 자체가 생기지 않습니다.
	if s.Len() != 0 {
		t.Fatalf("빈 상태가 아닙니다")
	}
}
