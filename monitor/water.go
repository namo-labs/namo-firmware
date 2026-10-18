package main

import (
	"context"
	"encoding/json"
	"errors"
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

// publishTimeout은 발행을 브로커가 확인해 주기를 기다리는 한계입니다.
//
// paho 토큰은 브로커가 응답하지 않으면 재접속할 때까지 끝나지 않습니다.
// 한계를 두지 않으면 요청이 영영 돌아오지 않아 웹이 "요청 중…"에서 멈춥니다.
var publishTimeout = 5 * time.Second

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
// 하므로 시각에 더해 카운터를 씁니다. 접두사는 누가 보냈는지를 사건
// 기록에 남기기 위한 것입니다(web · auto).
var idCounter struct {
	mu sync.Mutex
	n  uint64
}

func newID(prefix string) string {
	idCounter.mu.Lock()
	idCounter.n++
	n := idCounter.n
	idCounter.mu.Unlock()
	return fmt.Sprintf("%s%d-%d", prefix, time.Now().Unix(), n)
}

// resultTimeout은 장치가 급수 결과를 돌려주기를 기다리는 한계입니다.
var resultTimeout = 25 * time.Second

// 급수 명령을 보내다 실패한 이유. 호출하는 쪽이 사람에게 다르게 알립니다.
var (
	errBrokerDown    = errors.New("브로커에 연결돼 있지 않습니다")
	errBrokerSilent  = errors.New("브로커가 응답하지 않습니다")
	errPublishFailed = errors.New("명령을 보내지 못했습니다")
	// 명령은 나갔지만 결과를 못 봤습니다. 장치가 TTL로 버렸을 수도,
	// 결과만 유실됐을 수도 있습니다. 물이 나갔을 수 있으므로 곧바로
	// 다시 보내면 안 됩니다.
	errNoResult = errors.New("결과를 받지 못했습니다")
)

// sendWater는 급수 명령을 보내고 장치가 돌려준 결과를 그대로 넘깁니다.
//
// 웹 버튼과 자동급수가 같은 길을 씁니다. 어느 쪽이든 줘도 되는지는
// 장치가 판정합니다.
func sendWater(ctx context.Context, client mqtt.Client, deviceID string, p *pending, id string, dose int) (json.RawMessage, error) {
	// IsConnected가 아니라 IsConnectionOpen을 봅니다. 자동 재접속을 켜 두면
	// 브로커가 죽어 있는 동안에도 IsConnected는 true라, 명령이 큐에만
	// 쌓이고 결과는 오지 않습니다.
	if !client.IsConnectionOpen() {
		return nil, errBrokerDown
	}

	// TTL은 장치가 오래된 명령을 버리는 기준입니다(S8). 버튼이든 자동이든
	// 지금 줘야 해서 보낸 것이므로 짧게 잡습니다. 네트워크가 막혀 늦게
	// 도착한 명령이 한참 뒤에 물을 뿌리면 안 됩니다.
	payload, err := json.Marshal(map[string]any{
		"id":        id,
		"issued_at": time.Now().Unix(),
		"ttl_s":     30,
		"dose_ml":   dose,
	})
	if err != nil {
		return nil, fmt.Errorf("명령을 만들지 못했습니다: %w", err)
	}

	// 결과를 받을 자리를 먼저 만듭니다. 발행한 뒤에 만들면 그 사이에
	// 도착한 결과를 놓칩니다.
	ch := p.add(id)
	defer p.remove(id)

	tok := client.Publish("namo/pilot/"+deviceID+"/water/cmd", 1, false, payload)
	if !tok.WaitTimeout(publishTimeout) {
		// 명령이 큐에 남아 늦게 나갈 수 있지만, 장치가 TTL로 버립니다(S8).
		log.Printf("급수 명령 발행 확인 시간 초과 (id=%s)", id)
		return nil, errBrokerSilent
	}
	if err := tok.Error(); err != nil {
		log.Printf("급수 명령 발행 실패: %v", err)
		return nil, errPublishFailed
	}
	log.Printf("급수 요청: %dmL (id=%s)", dose, id)

	select {
	case raw := <-ch:
		return raw, nil
	case <-time.After(resultTimeout):
		return nil, errNoResult
	case <-ctx.Done():
		// 기다리던 쪽이 떠났습니다. 명령은 이미 나갔습니다.
		return nil, ctx.Err()
	}
}

// waterHandler는 급수 요청을 장치로 보내고 결과를 기다립니다.
func waterHandler(client mqtt.Client, deviceID string, p *pending, events *EventStore) func(w http.ResponseWriter, r *http.Request) {
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

		id := newID("web")
		raw, err := sendWater(r.Context(), client, deviceID, p, id, req.DoseML)
		switch {
		case err == nil:
		case errors.Is(err, errBrokerDown):
			writeJSON(w, http.StatusServiceUnavailable, map[string]any{"error": err.Error()})
			return
		case errors.Is(err, errBrokerSilent):
			writeJSON(w, http.StatusGatewayTimeout, map[string]any{"error": err.Error()})
			return
		case errors.Is(err, errPublishFailed):
			writeJSON(w, http.StatusBadGateway, map[string]any{"error": err.Error()})
			return
		case errors.Is(err, errNoResult):
			// 다시 보내라고 하면 두 번 나갈 수 있으므로 상태를 확인하라고
			// 안내합니다.
			writeJSON(w, http.StatusGatewayTimeout, map[string]any{
				"error": "결과를 받지 못했습니다. 상태를 확인하세요",
				"id":    id,
			})
			return
		case r.Context().Err() != nil:
			// 브라우저가 끊었습니다. 명령은 이미 나갔습니다.
			return
		default:
			writeJSON(w, http.StatusInternalServerError, map[string]any{"error": err.Error()})
			return
		}

		// 거부됐다면 왜 그런지 설명할 맥락을 함께 실어 보냅니다.
		// 화면이 "쿨다운"만 보여주면 언제 풀리는지 알 수 없습니다.
		var body map[string]any
		if err := json.Unmarshal(raw, &body); err != nil {
			writeJSON(w, http.StatusOK, raw)
			return
		}
		body["context"] = buildContext(events)
		writeJSON(w, http.StatusOK, body)
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
