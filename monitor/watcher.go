package main

import (
	"strconv"
	"sync"
)

// stateView는 텔레메트리에서 전이를 볼 필드만 추립니다.
type stateView struct {
	TS        *int64  `json:"ts"`
	Reservoir *string `json:"reservoir"`
	Leak      *string `json:"leak"`
	Locked    *bool   `json:"locked"`
	Gateway   *bool   `json:"gateway_online"`
	Sensors   *bool   `json:"leak_sensors_online"`
}

// watcher는 직전 상태를 들고 있다가 바뀐 것만 사건으로 남깁니다.
//
// 텔레메트리는 10초마다 오지만 대부분 같은 값입니다. 전부 남기면 목록이
// 의미 없는 줄로 가득 차므로, **바뀐 순간만** 기록합니다. 그래야 "물통이
// 언제 비었나"를 목록에서 바로 찾을 수 있습니다.
type watcher struct {
	mu   sync.Mutex
	prev stateView
	seen bool
}

func newWatcher() *watcher { return &watcher{} }

// observe는 새 상태를 보고 발생한 사건들을 돌려줍니다.
//
// 첫 관측에서는 사건을 내지 않습니다. 비교할 이전 값이 없어서, 서비스가
// 뜰 때마다 "물통 ok로 바뀜" 같은 가짜 전이가 생기기 때문입니다.
func (w *watcher) observe(cur stateView, now int64) []Event {
	w.mu.Lock()
	defer w.mu.Unlock()

	prev, seen := w.prev, w.seen
	w.prev, w.seen = cur, true
	if !seen {
		return nil
	}

	ts := now
	if cur.TS != nil {
		ts = *cur.TS
	}

	var out []Event
	add := func(kind, from, to string) {
		out = append(out, Event{TS: ts, Kind: kind, From: from, To: to})
	}

	if s := changedStr(prev.Reservoir, cur.Reservoir); s != nil {
		add(KindReservoir, s[0], s[1])
	}
	if s := changedStr(prev.Leak, cur.Leak); s != nil {
		add(KindLeak, s[0], s[1])
	}
	if s := changedBool(prev.Locked, cur.Locked); s != nil {
		add(KindLock, s[0], s[1])
	}
	if s := changedBool(prev.Gateway, cur.Gateway); s != nil {
		add(KindGateway, s[0], s[1])
	}
	if s := changedBool(prev.Sensors, cur.Sensors); s != nil {
		out = append(out, Event{TS: ts, Kind: KindGateway, From: "sensors:" + s[0], To: "sensors:" + s[1]})
	}
	return out
}

// changedStr는 값이 실제로 바뀌었을 때만 [이전, 이후]를 돌려줍니다.
//
// nil은 "아직 모름"이고 값이 아닙니다. nil에서 값으로 가는 것은 전이가
// 아니라 처음 알게 된 것이므로 사건으로 치지 않습니다.
func changedStr(a, b *string) []string {
	if a == nil || b == nil || *a == *b {
		return nil
	}
	return []string{*a, *b}
}

func changedBool(a, b *bool) []string {
	if a == nil || b == nil || *a == *b {
		return nil
	}
	return []string{strconv.FormatBool(*a), strconv.FormatBool(*b)}
}
