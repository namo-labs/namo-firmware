package main

import (
	"encoding/json"
	"fmt"
	"log"
	"net/http"
	"strconv"
	"sync"
	"time"

	mqtt "github.com/eclipse/paho.mqtt.golang"
)

// 웹에서 걸 수 있는 급수량. 장치의 1회 상한(300mL)보다 작아야 합니다.
//
// 오늘 실측으로 이 화분에는 100mL가 기준입니다. 200mL를 주자 흙수분이
// 83%까지 올라가 과습 영역에 들어갔습니다.
var allowedDoses = map[int]bool{50: true, 100: true, 200: true}

// pending은 보낸 급수 명령의 결과를 기다리는 곳입니다.
//
// 급수는 비동기입니다. 명령을 MQTT로 보내고, 결과는 장치가 별도 토픽으로
// 돌려줍니다. 웹 요청은 그 결과가 올 때까지 기다렸다가 응답합니다. 거부
// 사유를 그대로 보여주는 편이 "왜 안 나왔지"를 없앱니다.
type pending struct {
	mu      sync.Mutex
	waiting map[string]chan json.RawMessage
}

func newPending() *pending {
	return &pending{waiting: make(map[string]chan json.RawMessage)}
}

func (p *pending) add(id string) chan json.RawMessage {
	ch := make(chan json.RawMessage, 1)
	p.mu.Lock()
	p.waiting[id] = ch
	p.mu.Unlock()
	return ch
}

func (p *pending) remove(id string) {
	p.mu.Lock()
	delete(p.waiting, id)
	p.mu.Unlock()
}

// deliver는 결과를 기다리는 쪽에 넘깁니다. 기다리는 쪽이 없으면 버립니다.
func (p *pending) deliver(id string, raw json.RawMessage) {
	p.mu.Lock()
	ch, ok := p.waiting[id]
	p.mu.Unlock()
	if !ok {
		return
	}
	select {
	case ch <- raw:
	default: // 이미 값이 들어갔거나 대기가 끝났습니다.
	}
}

type waterRequest struct {
	DoseML int `json:"dose_ml"`
}

// newID는 명령 식별자를 만듭니다.
//
// 장치는 이 값으로 같은 명령의 재수신을 걸러냅니다(S9). 매번 달라야
// 하므로 시각에 더해 카운터를 씁니다.
var idCounter struct {
	mu sync.Mutex
	n  uint64
}

func newID() string {
	idCounter.mu.Lock()
	idCounter.n++
	n := idCounter.n
	idCounter.mu.Unlock()
	return fmt.Sprintf("web%d-%d", time.Now().Unix(), n)
}

// waterHandler는 급수 요청을 장치로 보내고 결과를 기다립니다.
func waterHandler(client mqtt.Client, deviceID string, p *pending, events *EventStore) func(w http.ResponseWriter, r *http.Request) {
	topicCmd := "namo/pilot/" + deviceID + "/water/cmd"

	return func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodPost {
			w.Header().Set("Allow", http.MethodPost)
			writeJSON(w, http.StatusMethodNotAllowed, map[string]any{
				"error": "POST만 받습니다",
			})
			return
		}

		if err := guardWrite(w, r); err != nil {
			return
		}

		var req waterRequest
		if err := json.NewDecoder(http.MaxBytesReader(w, r.Body, 1<<10)).Decode(&req); err != nil {
			writeJSON(w, http.StatusBadRequest, map[string]any{
				"error": "요청을 읽지 못했습니다",
			})
			return
		}
		if !allowedDoses[req.DoseML] {
			writeJSON(w, http.StatusBadRequest, map[string]any{
				"error": "허용되지 않은 급수량입니다",
			})
			return
		}

		if !client.IsConnected() {
			writeJSON(w, http.StatusServiceUnavailable, map[string]any{
				"error": "브로커에 연결돼 있지 않습니다",
			})
			return
		}

		id := newID()
		now := time.Now().Unix()
		// TTL은 장치가 오래된 명령을 버리는 기준입니다(S8). 사람이 버튼을
		// 누른 것이므로 짧게 잡습니다. 네트워크가 막혀 늦게 도착한 명령이
		// 한참 뒤에 물을 뿌리면 안 됩니다.
		payload, err := json.Marshal(map[string]any{
			"id":        id,
			"issued_at": now,
			"ttl_s":     30,
			"dose_ml":   req.DoseML,
		})
		if err != nil {
			writeJSON(w, http.StatusInternalServerError, map[string]any{
				"error": "명령을 만들지 못했습니다",
			})
			return
		}

		// 결과를 받을 자리를 먼저 만듭니다. 발행한 뒤에 만들면 그 사이에
		// 도착한 결과를 놓칩니다.
		ch := p.add(id)
		defer p.remove(id)

		if tok := client.Publish(topicCmd, 1, false, payload); tok.Wait() && tok.Error() != nil {
			log.Printf("급수 명령 발행 실패: %v", tok.Error())
			writeJSON(w, http.StatusBadGateway, map[string]any{
				"error": "명령을 보내지 못했습니다",
			})
			return
		}
		log.Printf("급수 요청: %dmL (id=%s)", req.DoseML, id)

		select {
		case raw := <-ch:
			// 거부됐다면 왜 그런지 설명할 맥락을 함께 실어 보냅니다.
			// 화면이 "쿨다운"만 보여주면 언제 풀리는지 알 수 없습니다.
			var body map[string]any
			if err := json.Unmarshal(raw, &body); err != nil {
				writeJSON(w, http.StatusOK, json.RawMessage(raw))
				return
			}
			body["context"] = buildContext(events)
			writeJSON(w, http.StatusOK, body)
		case <-time.After(25 * time.Second):
			// 명령은 나갔지만 결과를 못 봤습니다. 장치가 TTL로 버렸을 수도,
			// 결과만 유실됐을 수도 있습니다. 다시 보내라고 하면 두 번
			// 나갈 수 있으므로 상태를 확인하라고 안내합니다.
			writeJSON(w, http.StatusGatewayTimeout, map[string]any{
				"error": "결과를 받지 못했습니다. 상태를 확인하세요",
				"id":    id,
			})
		case <-r.Context().Done():
			// 브라우저가 끊었습니다. 명령은 이미 나갔습니다.
		}
	}
}

// sameOrigin은 Origin 헤더가 요청 호스트와 같은지 봅니다.
//
// 스킴과 포트까지 비교하지는 않습니다. 리버스 프록시 뒤에 있어 바깥은
// https, 안쪽은 http이기 때문입니다.
func sameOrigin(origin, host string) bool {
	h := stripScheme(origin)
	return h == host || stripPort(h) == stripPort(host)
}

func stripScheme(s string) string {
	for _, p := range []string{"https://", "http://"} {
		if len(s) > len(p) && s[:len(p)] == p {
			return s[len(p):]
		}
	}
	return s
}

func stripPort(s string) string {
	for i := len(s) - 1; i >= 0; i-- {
		if s[i] == ':' {
			if _, err := strconv.Atoi(s[i+1:]); err == nil {
				return s[:i]
			}
			return s
		}
	}
	return s
}
