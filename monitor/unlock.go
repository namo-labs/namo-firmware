package main

import (
	"encoding/json"
	"log"
	"net/http"
	"strings"
	"time"

	mqtt "github.com/eclipse/paho.mqtt.golang"
)

// unlockHandler는 누수로 걸린 잠금을 풉니다.
//
// 장치는 unlock에 결과를 돌려주지 않고 상태만 바꿉니다. 그래서 명령을
// 보낸 뒤 텔레메트리에서 `locked`가 내려가는 것을 보고 응답합니다.
// "보냈습니다"로 끝내면 정말 풀렸는지 사람이 다시 확인해야 합니다.
//
// **잠금은 누수를 본 뒤에만 걸립니다.** 원인을 확인하지 않고 푸는 것은
// 물이 새는 채로 급수를 다시 여는 것과 같으므로, 화면에서도 한 번 더
// 묻고 나서 부릅니다.
func unlockHandler(client mqtt.Client, deviceID string, st *state) func(http.ResponseWriter, *http.Request) {
	topic := "namo/pilot/" + deviceID + "/unlock"

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
		if !client.IsConnected() {
			writeJSON(w, http.StatusServiceUnavailable, map[string]any{
				"error": "브로커에 연결돼 있지 않습니다",
			})
			return
		}

		// 잠겨 있지 않으면 보낼 이유가 없습니다.
		if locked, known := lockedNow(st); known && !locked {
			writeJSON(w, http.StatusOK, map[string]any{
				"locked": false,
				"note":   "이미 풀려 있습니다",
			})
			return
		}

		id := newID()
		payload, _ := json.Marshal(map[string]any{"id": id})
		if tok := client.Publish(topic, 1, false, payload); tok.Wait() && tok.Error() != nil {
			log.Printf("unlock 발행 실패: %v", tok.Error())
			writeJSON(w, http.StatusBadGateway, map[string]any{
				"error": "명령을 보내지 못했습니다",
			})
			return
		}
		log.Printf("잠금 해제 요청 (id=%s)", id)

		// 텔레메트리 주기가 10초라 그보다 넉넉히 기다립니다.
		deadline := time.Now().Add(20 * time.Second)
		for time.Now().Before(deadline) {
			select {
			case <-r.Context().Done():
				return
			case <-time.After(500 * time.Millisecond):
			}
			if locked, known := lockedNow(st); known && !locked {
				writeJSON(w, http.StatusOK, map[string]any{"locked": false})
				return
			}
		}

		writeJSON(w, http.StatusGatewayTimeout, map[string]any{
			"error":  "잠금이 풀린 것을 확인하지 못했습니다",
			"locked": true,
		})
	}
}

// lockedNow는 마지막 텔레메트리의 잠금 상태를 봅니다.
// 두 번째 값은 아직 텔레메트리를 받지 못했으면 false입니다.
func lockedNow(st *state) (locked bool, known bool) {
	raw, _ := st.get()
	if raw == nil {
		return false, false
	}
	var v struct {
		Locked *bool `json:"locked"`
	}
	if err := json.Unmarshal(raw, &v); err != nil || v.Locked == nil {
		return false, false
	}
	return *v.Locked, true
}

// guardWrite는 상태를 바꾸는 요청에 공통으로 거는 방어입니다.
//
// Content-Type을 강제해 남의 페이지에 숨긴 폼으로는 보낼 수 없게 하고,
// 출처를 확인하되 **판단할 근거가 없으면 거부합니다**. 허용하는 쪽으로
// 기울면 헤더를 가리는 것만으로 검사를 지나칠 수 있습니다.
func guardWrite(w http.ResponseWriter, r *http.Request) error {
	if ct := r.Header.Get("Content-Type"); !strings.HasPrefix(ct, "application/json") {
		writeJSON(w, http.StatusUnsupportedMediaType, map[string]any{
			"error": "application/json으로 보내야 합니다",
		})
		return errRejected
	}

	origin := r.Header.Get("Origin")
	site := r.Header.Get("Sec-Fetch-Site")
	switch {
	case origin != "":
		if !sameOrigin(origin, r.Host) {
			writeJSON(w, http.StatusForbidden, map[string]any{
				"error": "다른 출처에서 온 요청입니다",
			})
			return errRejected
		}
	case site != "":
		if site != "same-origin" && site != "none" {
			writeJSON(w, http.StatusForbidden, map[string]any{
				"error": "다른 출처에서 온 요청입니다",
			})
			return errRejected
		}
	default:
		// 앱은 브라우저가 아니라 이 헤더들이 없습니다. 대신 Bearer
		// 토큰으로 자기를 밝히므로, 그 경우에는 통과시킵니다.
		if !strings.HasPrefix(r.Header.Get("Authorization"), "Bearer ") {
			writeJSON(w, http.StatusForbidden, map[string]any{
				"error": "출처를 확인할 수 없습니다",
			})
			return errRejected
		}
	}
	return nil
}

type rejected struct{}

func (rejected) Error() string { return "rejected" }

var errRejected = rejected{}
