package main

import (
	_ "embed"
	"encoding/json"
	"net/http"
	"strconv"
	"time"

	mqtt "github.com/eclipse/paho.mqtt.golang"
)

//go:embed index.html
var indexHTML []byte

// 크롬·파이어폭스는 HLS를 기본 지원하지 않아 이 라이브러리가 필요합니다.
// CDN 대신 함께 담아, 밖으로 나가지 않아도 재생되게 합니다.
//
//go:embed hls.min.js
var hlsJS []byte

func newRouter(st *state, store *Store, events *EventStore, client mqtt.Client, deviceID, apiToken string, pend *pending) http.Handler {
	mux := http.NewServeMux()

	mux.HandleFunc("/api/water", waterHandler(client, deviceID, pend, events))

	mux.HandleFunc("/hls.min.js", func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/javascript; charset=utf-8")
		w.Header().Set("Cache-Control", "public, max-age=86400")
		_, _ = w.Write(hlsJS)
	})

	// 사건 목록. 무슨 일이 언제 있었는지를 봅니다.
	mux.HandleFunc("/api/events", func(w http.ResponseWriter, r *http.Request) {
		limit := 100
		if v := r.URL.Query().Get("limit"); v != "" {
			if n, err := strconv.Atoi(v); err == nil && n > 0 && n <= 1000 {
				limit = n
			}
		}
		writeJSON(w, http.StatusOK, events.Recent(limit))
	})

	// Caddy가 /namo 접두사를 벗기고 넘기므로 여기서는 /stats로 옵니다.
	// 접두사 없이 직접 띄워 확인할 때를 위해 /도 같은 화면을 줍니다.
	mux.HandleFunc("/", func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/", "/stats", "/stats/":
			w.Header().Set("Content-Type", "text/html; charset=utf-8")
			w.Header().Set("Cache-Control", "no-store")
			_, _ = w.Write(indexHTML)
		default:
			http.NotFound(w, r)
		}
	})

	mux.HandleFunc("/api/state", func(w http.ResponseWriter, r *http.Request) {
		raw, received := st.get()
		if raw == nil {
			writeJSON(w, http.StatusServiceUnavailable, map[string]any{
				"error": "아직 텔레메트리를 받지 못했습니다",
			})
			return
		}
		// 장치가 조용해진 지 얼마나 됐는지를 함께 줍니다. 값이 멈춘 것과
		// 장치가 죽은 것을 화면에서 구분할 수 있어야 합니다.
		writeJSON(w, http.StatusOK, map[string]any{
			"state":          json.RawMessage(raw),
			"received_ago_s": int(time.Since(received).Seconds()),
			"context":        buildContext(events),
		})
	})

	mux.HandleFunc("/api/history", func(w http.ResponseWriter, r *http.Request) {
		hours := 24
		if v := r.URL.Query().Get("hours"); v != "" {
			if n, err := strconv.Atoi(v); err == nil && n > 0 && n <= 24*30 {
				hours = n
			}
		}
		since := time.Now().Add(-time.Duration(hours) * time.Hour).Unix()
		writeJSON(w, http.StatusOK, store.Since(since))
	})

	mux.HandleFunc("/healthz", func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusOK)
		_, _ = w.Write([]byte("ok"))
	})

	return withAuth(apiToken, mux)
}

func writeJSON(w http.ResponseWriter, code int, v any) {
	w.Header().Set("Content-Type", "application/json; charset=utf-8")
	w.Header().Set("Cache-Control", "no-store")
	w.WriteHeader(code)
	_ = json.NewEncoder(w).Encode(v)
}

// buildContext는 지금 상태를 설명하는 데 필요한 지난 사건들을 모읍니다.
//
// "쿨다운 중"만으로는 왜 막혔는지 알 수 없습니다. 직전에 언제 얼마나
// 줬는지, 물통은 언제 비었는지를 함께 봐야 판단할 수 있습니다.
func buildContext(events *EventStore) map[string]any {
	ctx := map[string]any{}

	if e, ok := events.LastWater("completed"); ok {
		ctx["last_watering"] = map[string]any{
			"ts": e.TS, "ml": e.ML, "pump_ms": e.PumpMS, "source": e.Source,
		}
	}
	if e, ok := events.LastWater(""); ok {
		ctx["last_attempt"] = map[string]any{
			"ts": e.TS, "status": e.Status, "reason": e.Reason, "ml": e.ML,
		}
	}
	if e, ok := events.LastTransition(KindReservoir, "empty"); ok {
		ctx["reservoir_empty_since"] = e.TS
	}
	if e, ok := events.LastTransition(KindLock, "true"); ok {
		ctx["locked_since"] = e.TS
	}
	if e, ok := events.LastTransition(KindGateway, "false"); ok {
		ctx["gateway_lost_at"] = e.TS
	}
	return ctx
}
