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

func newRouter(st *state, store *Store, client mqtt.Client, deviceID string, pend *pending) http.Handler {
	mux := http.NewServeMux()

	mux.HandleFunc("/api/water", waterHandler(client, deviceID, pend))

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
			"state":         json.RawMessage(raw),
			"received_ago_s": int(time.Since(received).Seconds()),
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

	return mux
}

func writeJSON(w http.ResponseWriter, code int, v any) {
	w.Header().Set("Content-Type", "application/json; charset=utf-8")
	w.Header().Set("Cache-Control", "no-store")
	w.WriteHeader(code)
	_ = json.NewEncoder(w).Encode(v)
}
