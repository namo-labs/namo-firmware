package main

import (
	"encoding/json"
	"path/filepath"
	"sort"
	"sync"
)

// Sample은 한 시점의 센서값입니다.
//
// 키가 짧은 것은 의도한 것입니다. 30일치를 파일 하나에 담으므로 필드 이름이
// 용량의 상당 부분을 차지합니다.
type Sample struct {
	TS       int64    `json:"ts"`
	Moisture *int     `json:"m,omitempty"`
	TempC    *float64 `json:"t,omitempty"`
	Lux      *int     `json:"l,omitempty"`
	EC       *int     `json:"e,omitempty"`
	// 이 시점에 급수된 양. 0이면 급수가 없었다는 뜻입니다.
	WaterML int `json:"w,omitempty"`
}

// Store는 센서 이력을 메모리에 두고 주기적으로 파일에 씁니다.
//
// 텔레메트리는 10초마다 오지만 흙수분은 분 단위로도 거의 변하지 않으므로,
// 일정 간격으로 솎아서 담습니다. 급수는 드물고 중요하므로 간격과 무관하게
// 즉시 남깁니다.
type Store struct {
	mu      sync.RWMutex
	samples []Sample
	path    string

	minGapS  int64
	maxAgeS  int64
	lastSave int64
	dirty    bool
}

func NewStore(dir string, minGapS, maxAgeS int64) *Store {
	s := &Store{
		path:    filepath.Join(dir, "history.json"),
		minGapS: minGapS,
		maxAgeS: maxAgeS,
	}
	s.load()
	return s
}

// AddSensor는 센서 표본을 기록합니다. 직전 기록과의 간격이 좁으면 버립니다.
func (s *Store) AddSensor(sample Sample) {
	s.mu.Lock()
	defer s.mu.Unlock()

	if n := len(s.samples); n > 0 {
		if sample.TS-s.samples[n-1].TS < s.minGapS {
			return
		}
	}
	s.samples = append(s.samples, sample)
	s.dirty = true
	s.trimLocked(sample.TS)
}

// AddWatering은 급수를 기록합니다. 간격 제한을 받지 않습니다.
func (s *Store) AddWatering(ts int64, ml int) {
	s.mu.Lock()
	defer s.mu.Unlock()

	s.samples = append(s.samples, Sample{TS: ts, WaterML: ml})
	s.dirty = true
	s.trimLocked(ts)
}

// Since는 주어진 시각 이후의 표본을 시간순으로 돌려줍니다.
func (s *Store) Since(ts int64) []Sample {
	s.mu.RLock()
	defer s.mu.RUnlock()

	i := sort.Search(len(s.samples), func(i int) bool {
		return s.samples[i].TS >= ts
	})
	out := make([]Sample, len(s.samples)-i)
	copy(out, s.samples[i:])
	return out
}

func (s *Store) trimLocked(now int64) {
	cutoff := now - s.maxAgeS
	i := sort.Search(len(s.samples), func(i int) bool {
		return s.samples[i].TS >= cutoff
	})
	if i > 0 {
		s.samples = append([]Sample(nil), s.samples[i:]...)
	}
}

// Flush는 바뀐 것이 있을 때만 파일에 씁니다.
//
// 먼저 임시 파일에 쓰고 바꿔치기합니다. 쓰는 도중에 죽어도 이전 파일이
// 남아 있어야 이력을 통째로 잃지 않습니다.
func (s *Store) Flush() error {
	s.mu.RLock()
	if !s.dirty {
		s.mu.RUnlock()
		return nil
	}
	data, err := json.Marshal(s.samples)
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

func (s *Store) load() {
	loadJSON(s.path, &s.samples)
}

func (s *Store) Len() int {
	s.mu.RLock()
	defer s.mu.RUnlock()
	return len(s.samples)
}
