package main

import (
	"encoding/json"
	"os"
	"path/filepath"
	"sort"
	"sync"
)

// 사건의 종류.
const (
	KindWater     = "water"     // 급수 시도 (성공·거부·중단 전부)
	KindReservoir = "reservoir" // 물통 상태가 바뀜
	KindLeak      = "leak"      // 누수 상태가 바뀜
	KindLock      = "lock"      // 잠김/풀림
	KindGateway   = "gateway"   // Zigbee 게이트웨이 생존
	KindDevice    = "device"    // 장치 자체의 online/offline
)

// Event는 "언제 무슨 일이 있었나"입니다.
//
// 센서 표본(Sample)이 시간에 따라 변하는 값이라면, 이쪽은 특정 시점에
// 일어난 사건입니다. 급수가 왜 거부됐는지, 물통이 언제 비었는지는
// 그래프가 아니라 사건 목록으로 봐야 알 수 있습니다.
type Event struct {
	TS   int64  `json:"ts"`
	Kind string `json:"k"`

	// 급수(KindWater)에서 씁니다.
	Status string `json:"s,omitempty"` // completed · rejected · aborted
	Reason string `json:"r,omitempty"`
	ML     int    `json:"ml,omitempty"`
	PumpMS int    `json:"pms,omitempty"`
	Source string `json:"src,omitempty"` // web이면 이 화면에서 보낸 것

	// 상태 전이에서 씁니다.
	From string `json:"f,omitempty"`
	To   string `json:"t,omitempty"`
}

// EventStore는 사건을 시간순으로 쌓고 파일에 남깁니다.
type EventStore struct {
	mu      sync.RWMutex
	events  []Event
	path    string
	maxAgeS int64
	dirty   bool
}

func NewEventStore(dir string, maxAgeS int64) *EventStore {
	s := &EventStore{
		path:    filepath.Join(dir, "events.json"),
		maxAgeS: maxAgeS,
	}
	loadJSON(s.path, &s.events)
	return s
}

func (s *EventStore) Add(e Event) {
	s.mu.Lock()
	defer s.mu.Unlock()

	s.events = append(s.events, e)
	s.dirty = true

	cutoff := e.TS - s.maxAgeS
	i := sort.Search(len(s.events), func(i int) bool {
		return s.events[i].TS >= cutoff
	})
	if i > 0 {
		s.events = append([]Event(nil), s.events[i:]...)
	}
}

// Recent는 최근 사건을 새것부터 돌려줍니다.
func (s *EventStore) Recent(limit int) []Event {
	s.mu.RLock()
	defer s.mu.RUnlock()

	if limit <= 0 || limit > len(s.events) {
		limit = len(s.events)
	}
	out := make([]Event, 0, limit)
	for i := len(s.events) - 1; i >= len(s.events)-limit; i-- {
		out = append(out, s.events[i])
	}
	return out
}

// LastWater는 조건에 맞는 가장 최근 급수를 찾습니다.
//
// 쿨다운이 걸렸을 때 "직전에 언제 얼마나 줬는지"를 함께 보여주기 위한
// 것입니다. 남은 시간만 알려주면 왜 막혔는지 알 수 없습니다.
func (s *EventStore) LastWater(status string) (Event, bool) {
	s.mu.RLock()
	defer s.mu.RUnlock()

	for i := len(s.events) - 1; i >= 0; i-- {
		e := s.events[i]
		if e.Kind == KindWater && (status == "" || e.Status == status) {
			return e, true
		}
	}
	return Event{}, false
}

// LastTransition은 특정 종류의 마지막 상태 전이를 찾습니다.
func (s *EventStore) LastTransition(kind, to string) (Event, bool) {
	s.mu.RLock()
	defer s.mu.RUnlock()

	for i := len(s.events) - 1; i >= 0; i-- {
		e := s.events[i]
		if e.Kind == kind && (to == "" || e.To == to) {
			return e, true
		}
	}
	return Event{}, false
}

func (s *EventStore) Flush() error {
	s.mu.RLock()
	if !s.dirty {
		s.mu.RUnlock()
		return nil
	}
	data, err := json.Marshal(s.events)
	s.mu.RUnlock()
	if err != nil {
		return err
	}
	if err := saveJSON(s.path, data); err != nil {
		return err
	}
	s.mu.Lock()
	s.dirty = false
	s.mu.Unlock()
	return nil
}

func (s *EventStore) Len() int {
	s.mu.RLock()
	defer s.mu.RUnlock()
	return len(s.events)
}

// saveJSON은 임시 파일에 쓰고 바꿔치기합니다.
//
// 쓰는 도중에 죽어도 이전 파일이 남아 있어야 이력을 통째로 잃지 않습니다.
func saveJSON(path string, data []byte) error {
	tmp := path + ".tmp"
	if err := os.WriteFile(tmp, data, 0o644); err != nil {
		return err
	}
	return os.Rename(tmp, path)
}

// loadJSON은 파일이 없거나 깨졌으면 조용히 넘어갑니다.
//
// 첫 기동에는 파일이 없고, 깨진 파일 때문에 서비스가 뜨지 못하는 편이
// 이력을 잃는 것보다 나쁩니다.
func loadJSON(path string, v any) {
	data, err := os.ReadFile(path)
	if err != nil {
		return
	}
	_ = json.Unmarshal(data, v)
}
