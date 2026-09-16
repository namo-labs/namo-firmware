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
	"strings"
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

	// 알림. 비어 있으면 알리지 않습니다.
	telegramToken  string
	telegramChatID string
	dryPct         int

	// 앱이 쓸 Bearer 토큰. 비어 있으면 Bearer 요청을 거부합니다.
	apiToken string
}

func loadConfig() config {
	return config{
		mqttURL:  env("MQTT_URL", "tcp://192.168.5.2:1883"),
		deviceID: env("DEVICE_ID", "pilot01"),
		dataDir:  env("DATA_DIR", "/data"),
		addr:     env("ADDR", ":8080"),
		minGapS:  envInt("MIN_GAP_S", 60),
		maxAgeS:  envInt("MAX_AGE_S", 30*24*3600),

		telegramToken:  env("TELOXIDE_TOKEN", ""),
		telegramChatID: env("TELEGRAM_CHAT_ID", ""),
		// 바질 적정이 40~60%입니다. 이 아래로 내려가면 물 줄 때가
		// 됐다는 신호로 봅니다.
		dryPct: int(envInt("DRY_PCT", 50)),

		apiToken: env("API_TOKEN", ""),
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
	ID          string `json:"id"`
	Status      string `json:"status"`
	Reason      string `json:"reason"`
	EstimatedML int    `json:"estimated_ml"`
	PumpMS      int    `json:"pump_ms"`
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
	events := NewEventStore(cfg.dataDir, cfg.maxAgeS)
	log.Printf("표본 %d개, 사건 %d개를 불러왔습니다", store.Len(), events.Len())

	st := &state{}
	pend := newPending()

	notifier := NewNotifier(cfg.telegramToken, cfg.telegramChatID)
	if notifier == nil {
		log.Print("알림 설정이 없습니다. 텔레그램으로 알리지 않습니다.")
	} else {
		log.Printf("알림 켜짐 (흙수분 기준 %d%%)", cfg.dryPct)
	}
	alerts := newAlertRules(notifier, cfg.dryPct)

	if cfg.apiToken == "" {
		log.Print("API_TOKEN이 없습니다. Bearer 토큰 요청은 모두 거부됩니다.")
	}

	client := connectMQTT(cfg, st, store, events, pend, alerts)

	srv := &http.Server{
		Addr:    cfg.addr,
		Handler: newRouter(st, store, events, client, cfg.deviceID, cfg.apiToken, pend),
		// 급수 요청은 장치 결과를 기다리므로 응답이 오래 걸립니다.
		ReadHeaderTimeout: 5 * time.Second,
		WriteTimeout:      40 * time.Second,
	}

	// 장치가 조용해진 것은 메시지가 오지 않는 것이라 핸들러로는 잡을 수
	// 없습니다. 따로 시계를 보고 확인합니다.
	go func() {
		t := time.NewTicker(time.Minute)
		defer t.Stop()
		for range t.C {
			_, received := st.get()
			alerts.checkQuiet(received, time.Now())
		}
	}()

	// 주기적으로 저장합니다. 죽어도 이 주기만큼만 잃습니다.
	ticker := time.NewTicker(5 * time.Minute)
	defer ticker.Stop()
	go func() {
		for range ticker.C {
			if err := store.Flush(); err != nil {
				log.Printf("표본 저장 실패: %v", err)
			}
			if err := events.Flush(); err != nil {
				log.Printf("사건 저장 실패: %v", err)
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
		log.Printf("마지막 표본 저장 실패: %v", err)
	}
	if err := events.Flush(); err != nil {
		log.Printf("마지막 사건 저장 실패: %v", err)
	}
}

func connectMQTT(cfg config, st *state, store *Store, events *EventStore, pend *pending, alerts *alertRules) mqtt.Client {
	base := "namo/pilot/" + cfg.deviceID
	topicTelemetry := base + "/telemetry"
	topicResult := base + "/water/result"
	topicStatus := base + "/status"
	w := newWatcher()

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
				handleTelemetry(m.Payload(), st, store, events, w, alerts)
			},
			topicResult: func(_ mqtt.Client, m mqtt.Message) {
				handleWaterResult(m.Payload(), store, events, pend)
			},
			// 장치가 죽으면 LWT로 offline이 옵니다. 언제 끊겼는지는
			// 사건으로 남겨야 나중에 되짚을 수 있습니다.
			topicStatus: func(_ mqtt.Client, m mqtt.Message) {
				handleStatus(string(m.Payload()), events)
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

func handleTelemetry(payload []byte, st *state, store *Store, events *EventStore, w *watcher, alerts *alertRules) {
	var t telemetry
	if err := json.Unmarshal(payload, &t); err != nil {
		log.Printf("텔레메트리 파싱 실패: %v", err)
		return
	}
	st.set(payload)

	// 알릴 것이 있는지 봅니다.
	var as alertState
	if err := json.Unmarshal(payload, &as); err == nil {
		alerts.check(as, time.Now())
	}

	// 상태가 바뀐 순간만 사건으로 남깁니다.
	var sv stateView
	if err := json.Unmarshal(payload, &sv); err == nil {
		for _, e := range w.observe(sv, time.Now().Unix()) {
			log.Printf("상태 변화: %s %s → %s", e.Kind, e.From, e.To)
			events.Add(e)
		}
	}

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

func handleWaterResult(payload []byte, store *Store, events *EventStore, pend *pending) {
	var r waterResult
	if err := json.Unmarshal(payload, &r); err != nil {
		log.Printf("급수 결과 파싱 실패: %v", err)
		return
	}
	// 웹에서 보낸 명령이면 기다리는 요청에 결과를 넘깁니다.
	if r.ID != "" {
		pend.deliver(r.ID, append(json.RawMessage(nil), payload...))
	}

	ts := time.Now().Unix()
	if r.FinishedAt != nil {
		ts = *r.FinishedAt
	}

	// **거부와 중단도 남깁니다.** 왜 물이 안 나갔는지는 성공 기록만으로는
	// 알 수 없고, 그걸 알아야 쿨다운인지 물통이 빈 것인지 되짚습니다.
	src := ""
	if strings.HasPrefix(r.ID, "web") {
		src = "web"
	}
	events.Add(Event{
		TS: ts, Kind: KindWater,
		Status: r.Status, Reason: r.Reason,
		ML: r.EstimatedML, PumpMS: r.PumpMS, Source: src,
	})
	log.Printf("급수 결과: %s %s (%dmL)", r.Status, r.Reason, r.EstimatedML)

	// 그래프의 세로선은 실제로 물이 나간 것만 찍습니다.
	if r.EstimatedML > 0 {
		store.AddWatering(ts, r.EstimatedML)
	}
}

func handleStatus(payload string, events *EventStore) {
	st := strings.TrimSpace(payload)
	if st != "online" && st != "offline" {
		return
	}
	log.Printf("장치 상태: %s", st)
	events.Add(Event{TS: time.Now().Unix(), Kind: KindDevice, To: st})
}
