// namo-monitor는 파일럿의 텔레메트리를 모아 웹으로 보여줍니다.
//
// ESP32도 상태 페이지를 갖고 있지만 그것은 지금 값만 보여주고, 집 밖에서는
// 볼 수 없습니다. 식물을 키울 때 정작 필요한 것은 "지금 몇 퍼센트"보다
// "물을 준 뒤로 어떻게 말라왔나"입니다. 그래서 이력을 쌓습니다.
//
// ESP32를 직접 외부에 노출하지 않는 이유이기도 합니다. 작은 기기에 공개
// 트래픽이 꽂히면 부담이고, 재부팅 중에는 아예 응답하지 못합니다.
package main

import (
	"context"
	"encoding/json"
	"errors"
	"log"
	"net/http"
	"os"
	"os/signal"
	"strconv"
	"sync"
	"syscall"
	"time"

	mqtt "github.com/eclipse/paho.mqtt.golang"
)

type config struct {
	mqttURL  string
	deviceID string
	dataDir  string
	addr     string
	// 표본을 남기는 최소 간격. 텔레메트리는 10초마다 오지만 흙수분은
	// 분 단위로도 거의 변하지 않습니다.
	minGapS int64
	maxAgeS int64
}

func loadConfig() config {
	return config{
		mqttURL:  env("MQTT_URL", "tcp://192.168.5.2:1883"),
		deviceID: env("DEVICE_ID", "pilot01"),
		dataDir:  env("DATA_DIR", "/data"),
		addr:     env("ADDR", ":8080"),
		minGapS:  envInt("MIN_GAP_S", 60),
		maxAgeS:  envInt("MAX_AGE_S", 30*24*3600),
	}
}

func env(key, def string) string {
	if v := os.Getenv(key); v != "" {
		return v
	}
	return def
}

func envInt(key string, def int64) int64 {
	if v := os.Getenv(key); v != "" {
		if n, err := strconv.ParseInt(v, 10, 64); err == nil {
			return n
		}
	}
	return def
}

// state는 마지막으로 받은 텔레메트리를 그대로 보관합니다.
//
// 필드를 해석하지 않고 통째로 들고 있습니다. 펌웨어가 필드를 추가해도
// 이쪽을 고치지 않아도 되고, 계약이 어긋날 여지도 줄어듭니다.
type state struct {
	mu       sync.RWMutex
	raw      json.RawMessage
	received time.Time
}

func (s *state) set(raw json.RawMessage) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.raw = append(json.RawMessage(nil), raw...)
	s.received = time.Now()
}

func (s *state) get() (json.RawMessage, time.Time) {
	s.mu.RLock()
	defer s.mu.RUnlock()
	return s.raw, s.received
}

// telemetry는 이력에 담을 필드만 추립니다.
type telemetry struct {
	TS       *int64   `json:"ts"`
	Moisture *int     `json:"soil_moisture_pct"`
	TempC    *float64 `json:"soil_temperature_c"`
	Lux      *int     `json:"illuminance_lux"`
	EC       *int     `json:"soil_conductivity_us_cm"`
}

type waterResult struct {
	Status      string `json:"status"`
	EstimatedML int    `json:"estimated_ml"`
	FinishedAt  *int64 `json:"finished_at"`
}

func main() {
	log.SetFlags(log.LstdFlags | log.Lmsgprefix)
	log.SetPrefix("namo-monitor ")

	cfg := loadConfig()
	if err := os.MkdirAll(cfg.dataDir, 0o755); err != nil {
		log.Fatalf("데이터 디렉토리를 만들지 못했습니다: %v", err)
	}

	store := NewStore(cfg.dataDir, cfg.minGapS, cfg.maxAgeS)
	log.Printf("이력 %d개를 불러왔습니다", store.Len())

	st := &state{}
	client := connectMQTT(cfg, st, store)

	srv := &http.Server{
		Addr:              cfg.addr,
		Handler:           newRouter(st, store),
		ReadHeaderTimeout: 5 * time.Second,
	}

	// 주기적으로 저장합니다. 죽어도 이 주기만큼만 잃습니다.
	ticker := time.NewTicker(5 * time.Minute)
	defer ticker.Stop()
	go func() {
		for range ticker.C {
			if err := store.Flush(); err != nil {
				log.Printf("이력 저장 실패: %v", err)
			}
		}
	}()

	go func() {
		log.Printf("HTTP %s에서 듣습니다", cfg.addr)
		if err := srv.ListenAndServe(); err != nil && !errors.Is(err, http.ErrServerClosed) {
			log.Fatalf("HTTP 서버가 멈췄습니다: %v", err)
		}
	}()

	stop := make(chan os.Signal, 1)
	signal.Notify(stop, syscall.SIGINT, syscall.SIGTERM)
	<-stop

	log.Print("종료합니다")
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	_ = srv.Shutdown(ctx)
	client.Disconnect(250)
	if err := store.Flush(); err != nil {
		log.Printf("마지막 저장 실패: %v", err)
	}
}

func connectMQTT(cfg config, st *state, store *Store) mqtt.Client {
	base := "namo/pilot/" + cfg.deviceID
	topicTelemetry := base + "/telemetry"
	topicResult := base + "/water/result"

	opts := mqtt.NewClientOptions().
		AddBroker(cfg.mqttURL).
		SetClientID("namo-monitor-" + strconv.FormatInt(time.Now().UnixNano(), 36)).
		SetAutoReconnect(true).
		SetConnectRetry(true).
		SetConnectRetryInterval(10 * time.Second).
		SetKeepAlive(30 * time.Second)

	// 구독은 접속 콜백에서 합니다. 재접속할 때마다 다시 걸어야 하기
	// 때문입니다. 브로커가 세션을 잊으면 구독도 함께 사라집니다.
	opts.SetOnConnectHandler(func(c mqtt.Client) {
		log.Print("MQTT 접속됨")
		for topic, handler := range map[string]mqtt.MessageHandler{
			topicTelemetry: func(_ mqtt.Client, m mqtt.Message) {
				handleTelemetry(m.Payload(), st, store)
			},
			topicResult: func(_ mqtt.Client, m mqtt.Message) {
				handleWaterResult(m.Payload(), store)
			},
		} {
			if tok := c.Subscribe(topic, 1, handler); tok.Wait() && tok.Error() != nil {
				log.Printf("구독 실패 (%s): %v", topic, tok.Error())
			} else {
				log.Printf("구독: %s", topic)
			}
		}
	})
	opts.SetConnectionLostHandler(func(_ mqtt.Client, err error) {
		log.Printf("MQTT 끊김: %v", err)
	})

	client := mqtt.NewClient(opts)
	// ConnectRetry가 켜져 있으므로 첫 접속이 실패해도 계속 시도합니다.
	// 브로커가 늦게 뜨는 경우에 여기서 죽으면 안 됩니다.
	client.Connect()
	return client
}

func handleTelemetry(payload []byte, st *state, store *Store) {
	var t telemetry
	if err := json.Unmarshal(payload, &t); err != nil {
		log.Printf("텔레메트리 파싱 실패: %v", err)
		return
	}
	st.set(payload)

	// 장치 시각이 없으면 아직 시각 동기화 전입니다. 이력에는 담지
	// 않습니다. 시각을 모르는 표본은 가로축에 놓을 자리가 없습니다.
	if t.TS == nil {
		return
	}
	store.AddSensor(Sample{
		TS:       *t.TS,
		Moisture: t.Moisture,
		TempC:    t.TempC,
		Lux:      t.Lux,
		EC:       t.EC,
	})
}

func handleWaterResult(payload []byte, store *Store) {
	var r waterResult
	if err := json.Unmarshal(payload, &r); err != nil {
		log.Printf("급수 결과 파싱 실패: %v", err)
		return
	}
	// 거부되거나 중단된 급수도 물이 나갔을 수 있으므로 양으로 판단합니다.
	if r.EstimatedML <= 0 || r.FinishedAt == nil {
		return
	}
	log.Printf("급수 기록: %dmL (%s)", r.EstimatedML, r.Status)
	store.AddWatering(*r.FinishedAt, r.EstimatedML)
}
