package main

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"log"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"

	mqtt "github.com/eclipse/paho.mqtt.golang"
)

// 자동급수 경보. 같은 이름은 한 번만 올라가고 한 번만 내려갑니다.
const (
	alertAutoHold    = "auto-hold"    // 물통·누수·잠김·한도로 주지 못함
	alertAutoFail    = "auto-fail"    // 장치가 거부·중단했거나 결과를 못 받음
	alertAutoStopped = "auto-stopped" // 줬는데 흙수분이 오르지 않아 멈춤
)

const (
	// 장치가 재부팅해 쿨다운(S6)을 잊어도 연달아 주지 않도록 이쪽에서도
	// 간격을 둡니다. 장치 쿨다운과 같은 30분입니다.
	autoMinGap = 30 * time.Minute
	// HHCC는 네 값을 하나씩 돌아가며 광고해 흙수분이 40초쯤마다
	// 갱신됩니다. 그보다 넉넉히 잡습니다.
	autoSensorFreshS = 300
	// 급수가 흙수분에 반영되기까지 약 4분 걸립니다. 그보다 넉넉히 봅니다.
	autoEffectWindow = 15 * time.Minute
	autoMinRisePct   = 3
	// 이만큼 회복하면 사람이 손을 쓴 것으로 보고 멈춤을 풉니다. 마름
	// 경보의 해제 기준과 같습니다.
	autoResumeMargin = 10
)

// 장치가 돌려주는 사유를 사람 말로 바꿉니다. 화면(index.html)의 표와 같은
// 뜻입니다. 주기 전에 텔레메트리로 걸러낸 이유도 같은 이름을 씁니다.
var reasonText = map[string]string{
	"cooldown":          "쿨다운 중입니다",
	"daily_limit":       "오늘 한도를 다 썼습니다",
	"reservoir_empty":   "물통이 비었습니다",
	"reservoir_unknown": "물통 상태를 모릅니다",
	"leak_detected":     "누수가 감지됐습니다",
	"leak_stale":        "누수 상태를 믿을 수 없습니다",
	"locked":            "누수로 잠겨 있습니다",
	"dose_too_large":    "요청량이 상한을 넘습니다",
	"already_running":   "이미 급수 중입니다",
	"clock_unsynced":    "장치 시각이 맞춰지지 않았습니다",
	"hardware_error":    "하드웨어 오류입니다",
}

func reasonLabel(code string) string {
	if t, ok := reasonText[code]; ok {
		return t
	}
	return code
}

// autoView는 자동급수 판단에 필요한 필드만 봅니다.
type autoView struct {
	TS            *int64  `json:"ts"`
	Moisture      *int    `json:"soil_moisture_pct"`
	SensorAgo     *int    `json:"sensor_seen_ago_s"`
	Reservoir     *string `json:"reservoir"`
	Leak          *string `json:"leak"`
	Locked        *bool   `json:"locked"`
	Pump          *string `json:"pump"`
	Today         *int    `json:"today_estimated_ml"`
	CooldownUntil *int64  `json:"cooldown_until"`
}

// autoPlan은 보내기로 정한 급수 한 건입니다.
type autoPlan struct {
	dose     int
	moisture int // 보내기로 정한 때의 흙수분. 효과를 이것과 비교합니다.
	today    int
	at       time.Time
}

// autoWaterer는 흙이 마르면 스스로 물을 줍니다(pilot-design.md §7).
//
// **언제 줄지만 정합니다.** 줘도 되는지는 장치가 판정합니다. 명령은 웹
// 버튼과 같은 길로 나가 쿨다운·일일 한도·물통·누수 규칙을 그대로 거칩니다.
// 여기서 텔레메트리로 미리 거르는 것은 거부될 명령을 보내지 않고, 왜 못
// 주는지 사람에게 알리기 위해서입니다.
type autoWaterer struct {
	mu   sync.Mutex
	n    *Notifier
	path string

	enabled bool
	dryPct  int
	doseML  int

	// 마지막으로 명령을 보낸 시각. 결과와 무관하게 씁니다. 결과를 못
	// 받았어도 물이 나갔을 수 있습니다.
	lastSentAt time.Time

	// 효과 확인. 완료된 급수 뒤에만 켭니다.
	checking   bool
	checkSince time.Time
	before     int
	peak       int

	// 줬는데 흙수분이 오르지 않았거나 장치가 하드웨어 오류를 내서
	// 멈췄습니다. 흙이 충분히 젖거나 사람이 다시 맡겨야 풀립니다.
	// 안전장치라 파일에 남깁니다. 재시작했다고 풀리면 안 됩니다.
	stopped bool
	// 지금 주지 못하는 이유(사유 코드). 상태를 보여주기 위한 것입니다.
	hold string
}

type autoSaved struct {
	Enabled bool `json:"enabled"`
	Stopped bool `json:"stopped"`
}

// newAutoWaterer는 저장된 켜짐·멈춤을 읽습니다. 파일이 없을 때만
// defaultOn을 씁니다. 화면에서 끈 것이 재시작으로 되살아나면 안 됩니다.
func newAutoWaterer(n *Notifier, dir string, defaultOn bool, dryPct, doseML int) *autoWaterer {
	a := &autoWaterer{
		n:       n,
		path:    filepath.Join(dir, "auto.json"),
		enabled: defaultOn,
		dryPct:  dryPct,
		doseML:  doseML,
	}
	if data, err := os.ReadFile(a.path); err == nil {
		var saved autoSaved
		if err := json.Unmarshal(data, &saved); err == nil {
			a.enabled, a.stopped = saved.Enabled, saved.Stopped
		}
	}
	return a
}

func (a *autoWaterer) Enabled() bool {
	a.mu.Lock()
	defer a.mu.Unlock()
	return a.enabled
}

// saveLocked는 켜짐·멈춤을 파일에 남깁니다. a.mu를 잡은 채로 부릅니다.
func (a *autoWaterer) saveLocked() error {
	data, err := json.Marshal(autoSaved{Enabled: a.enabled, Stopped: a.stopped})
	if err != nil {
		return err
	}
	return saveJSON(a.path, data)
}

// SetEnabled는 켜고 끕니다.
//
// 값이 그대로면 아무것도 바꾸지 않습니다. 앱이 상태를 맞추려고 같은 값을
// 보냈다고 멈춤이 풀리면 안 됩니다. 값이 바뀌면 사람이 상황을 보고 다시
// 맡긴 것이므로 멈춤과 지난 경보를 풉니다. 마지막으로 보낸 시각은 그대로
// 둬, 껐다 켜는 것으로 30분 간격을 건너뛰지 못하게 합니다.
//
// 저장하지 못하면 켜기는 거부하고 끄기는 그대로 먹입니다. 켜졌다고
// 보이는데 재시작하면 꺼지는 것보다, 물을 멈추지 못하는 쪽이 더 나쁩니다.
func (a *autoWaterer) SetEnabled(on bool) error {
	a.mu.Lock()
	if a.enabled == on {
		a.mu.Unlock()
		return nil
	}
	prevStopped := a.stopped
	a.enabled, a.stopped = on, false
	err := a.saveLocked()
	if err != nil && on {
		a.enabled, a.stopped = false, prevStopped
		a.mu.Unlock()
		return err
	}
	a.checking = false
	a.hold = ""
	a.mu.Unlock()

	for _, key := range []string{alertAutoHold, alertAutoFail, alertAutoStopped} {
		a.n.Clear(key, "")
	}
	return err
}

// Resume은 멈춘 자동급수를 다시 맡깁니다. 배선이나 센서를 확인한 뒤
// 화면에서 "다시 켜기"를 누를 때 부릅니다. 저장하지 못해도 풀되, 재시작하면
// 다시 멈춘 상태로 돌아갑니다. 안전한 쪽으로 돌아가는 것이라 막지 않습니다.
func (a *autoWaterer) Resume() error {
	a.mu.Lock()
	if !a.stopped {
		a.mu.Unlock()
		return nil
	}
	a.stopped = false
	a.checking = false
	err := a.saveLocked()
	a.mu.Unlock()

	a.n.Clear(alertAutoStopped, "")
	return err
}

// stopLocked는 자동급수를 멈추고 남깁니다. a.mu를 잡은 채로 부릅니다.
func (a *autoWaterer) stopLocked(text string, out *outbox) {
	a.checking = false
	a.stopped = true
	if err := a.saveLocked(); err != nil {
		log.Printf("자동급수 멈춤을 저장하지 못했습니다: %v", err)
	}
	out.add(func() { a.n.Raise(alertAutoStopped, text) })
}

// outbox는 잠금을 놓은 뒤에 보낼 알림을 모읍니다. 텔레그램이 느리면 그동안
// 텔레메트리 처리와 상태 조회가 막히기 때문입니다.
type outbox []func()

func (o *outbox) add(f func()) { *o = append(*o, f) }

func (o *outbox) flush() {
	for _, f := range *o {
		f()
	}
}

// decide는 텔레메트리 한 번을 보고 지금 물을 줄지 정합니다. 주기로 했으면
// 보낸 것으로 기록하므로, 돌려받은 계획은 반드시 execute로 실행합니다.
func (a *autoWaterer) decide(v autoView, now time.Time) (autoPlan, bool) {
	var out outbox
	defer out.flush()
	a.mu.Lock()
	defer a.mu.Unlock()

	if !a.enabled {
		return autoPlan{}, false
	}

	m, fresh := 0, false
	if v.Moisture != nil && v.SensorAgo != nil && *v.SensorAgo <= autoSensorFreshS {
		m, fresh = *v.Moisture, true
	}

	a.checkEffect(m, fresh, now, &out)

	if !fresh {
		return autoPlan{}, false
	}
	if m >= a.dryPct {
		a.hold = ""
		// 막던 이유가 풀렸으면 보류 경보를 내려, 다음에 다른 이유로
		// 막히면 다시 알리게 합니다. 이유가 그대로면 경계에서 흔들려도
		// 다시 알리지 않습니다.
		if a.blocked(v) == "" {
			out.add(func() { a.n.Clear(alertAutoHold, "") })
		}
	}
	if m >= a.dryPct+autoResumeMargin {
		a.resume(m, &out)
		return autoPlan{}, false
	}
	if m >= a.dryPct || a.stopped || a.checking {
		return autoPlan{}, false
	}
	if !a.lastSentAt.IsZero() && now.Sub(a.lastSentAt) < autoMinGap {
		return autoPlan{}, false
	}

	// 곧 풀리는 것들은 알리지 않고 기다립니다.
	if v.TS == nil || v.CooldownUntil != nil || v.Pump == nil || *v.Pump != "idle" || v.Today == nil {
		return autoPlan{}, false
	}

	if why := a.blocked(v); why != "" {
		a.hold = why
		text := strings.Join([]string{
			fmt.Sprintf("⏸ <b>자동급수를 못 하고 있습니다 — 흙 %d%%</b>", m),
			"",
			reasonLabel(why) + ".",
			"풀리면 다시 시도합니다.",
		}, "\n")
		out.add(func() { a.n.Raise(alertAutoHold, text) })
		return autoPlan{}, false
	}

	a.hold = ""
	a.lastSentAt = now
	out.add(func() { a.n.Clear(alertAutoHold, "") })
	return autoPlan{dose: a.doseML, moisture: m, today: *v.Today, at: now}, true
}

// blocked는 사람이 손을 써야 풀리는 이유를 사유 코드로 돌려줍니다.
func (a *autoWaterer) blocked(v autoView) string {
	switch {
	case v.Reservoir == nil || *v.Reservoir == "unknown":
		return "reservoir_unknown"
	case *v.Reservoir != "ok":
		return "reservoir_empty"
	case v.Leak != nil && *v.Leak == "detected":
		return "leak_detected"
	case v.Leak == nil || *v.Leak != "none":
		return "leak_stale"
	case v.Locked == nil || *v.Locked:
		return "locked"
	case v.Today != nil && *v.Today+a.doseML > deviceDailyLimitML:
		return "daily_limit"
	}
	return ""
}

// checkEffect는 완료된 급수 뒤 흙수분이 실제로 올랐는지 봅니다.
//
// completed는 GPIO까지만 보증합니다. 모터 선이 빠져 있어도 completed가
// 옵니다(2026-10-13). 오래된 센서값은 올랐어도 믿지 않습니다.
func (a *autoWaterer) checkEffect(m int, fresh bool, now time.Time, out *outbox) {
	if !a.checking {
		return
	}
	if fresh && m > a.peak {
		a.peak = m
	}
	if a.peak-a.before >= autoMinRisePct {
		a.checking = false
		return
	}
	if now.Sub(a.checkSince) < autoEffectWindow {
		return
	}

	text := strings.Join([]string{
		"⚠️ <b>자동급수를 멈췄습니다</b>",
		"",
		fmt.Sprintf("%dmL를 줬는데 %d분 동안 흙수분이 오르지 않았습니다 (%d%% → 최고 %d%%).",
			a.doseML, int(autoEffectWindow.Minutes()), a.before, a.peak),
		"물이 실제로 나가지 않았거나 센서가 흙에서 빠졌을 수 있습니다.",
		"펌프 배선과 센서를 확인하세요.",
		"",
		a.resumeHint(),
	}, "\n")
	a.stopLocked(text, out)
}

func (a *autoWaterer) resumeHint() string {
	return fmt.Sprintf("흙이 %d%% 이상으로 젖거나 화면에서 \"다시 켜기\"를 누르면 재개합니다.", a.dryPct+autoResumeMargin)
}

// resume은 흙이 충분히 젖었을 때 멈춤과 보류를 풉니다.
func (a *autoWaterer) resume(m int, out *outbox) {
	wasStopped := a.stopped
	a.stopped = false
	if wasStopped {
		if err := a.saveLocked(); err != nil {
			log.Printf("자동급수 재개를 저장하지 못했습니다: %v", err)
		}
	}
	out.add(func() {
		a.n.Clear(alertAutoHold, "")
		if wasStopped {
			a.n.Clear(alertAutoStopped, fmt.Sprintf("✅ <b>흙이 젖어 자동급수를 다시 켭니다 — %d%%</b>", m))
		}
	})
}

// execute는 계획을 보내고 결과를 알립니다. 결과를 기다리느라 수십 초
// 걸리므로 텔레메트리 처리와 다른 고루틴에서 부릅니다.
func (a *autoWaterer) execute(p autoPlan, send func(dose int) (waterResult, error)) {
	res, err := send(p.dose)

	var out outbox
	defer out.flush()
	a.mu.Lock()
	defer a.mu.Unlock()

	switch {
	case errors.Is(err, errNoResult), errors.Is(err, errBrokerSilent):
		// 발행을 확인받지 못한 명령도 늦게 나가 TTL 안에 닿을 수 있습니다.
		log.Printf("자동급수 결과 없음: %v", err)
		text := strings.Join([]string{
			"⚠️ <b>자동급수 결과를 받지 못했습니다</b>",
			"",
			err.Error() + ".",
			"물이 나갔는지 모르므로 30분 뒤에 다시 봅니다.",
		}, "\n")
		out.add(func() { a.n.Raise(alertAutoFail, text) })

	case err != nil:
		// 명령이 장치에 닿지 않았습니다. 늦게 닿더라도 장치가 TTL로
		// 버립니다(S8).
		log.Printf("자동급수 명령 실패: %v", err)
		text := strings.Join([]string{
			"⚠️ <b>자동급수 명령을 보내지 못했습니다</b>",
			"",
			err.Error() + ".",
			"30분 뒤에 다시 시도합니다.",
		}, "\n")
		out.add(func() { a.n.Raise(alertAutoFail, text) })

	case res.Status == "completed":
		log.Printf("자동급수 완료: %dmL (흙수분 %d%%)", res.EstimatedML, p.moisture)
		a.checking = true
		a.checkSince = p.at
		a.before, a.peak = p.moisture, p.moisture
		text := strings.Join([]string{
			fmt.Sprintf("💧 <b>자동급수 %dmL</b>", res.EstimatedML),
			"",
			fmt.Sprintf("흙수분이 %d%%까지 내려가 물을 줬습니다.", p.moisture),
			fmt.Sprintf("오늘 급수: %dmL / %dmL", p.today+res.EstimatedML, deviceDailyLimitML),
		}, "\n")
		out.add(func() {
			a.n.Clear(alertAutoFail, "")
			a.n.Notify(text)
		})

	case res.Status == "failed":
		// 같은 오류를 30분마다 다시 부를 이유가 없습니다. 사람이 봐야 합니다.
		log.Printf("자동급수 하드웨어 오류: %s", res.Reason)
		a.stopLocked(strings.Join([]string{
			"⚠️ <b>자동급수를 멈췄습니다</b>",
			"",
			"장치가 " + reasonLabel(res.Reason) + "라고 답했습니다.",
			"펌프와 배선을 확인하세요.",
			"",
			a.resumeHint(),
		}, "\n"), &out)

	default:
		log.Printf("자동급수 %s: %s (%dmL)", res.Status, res.Reason, res.EstimatedML)
		what := "거부됐습니다"
		if res.Status == "aborted" {
			what = "중단됐습니다"
		}
		lines := []string{
			fmt.Sprintf("⚠️ <b>자동급수가 %s</b>", what),
			"",
			reasonLabel(res.Reason) + ".",
		}
		if res.EstimatedML > 0 {
			lines = append(lines, fmt.Sprintf("중단 전에 %dmL가 나갔습니다.", res.EstimatedML))
		}
		lines = append(lines, "30분 뒤에 다시 시도합니다.")
		text := strings.Join(lines, "\n")
		out.add(func() { a.n.Raise(alertAutoFail, text) })
	}
}

// observe는 텔레메트리를 보고, 줄 때가 됐으면 명령을 보냅니다.
func (a *autoWaterer) observe(payload []byte, now time.Time, send func(dose int) (waterResult, error)) {
	var v autoView
	if err := json.Unmarshal(payload, &v); err != nil {
		return
	}
	if p, ok := a.decide(v, now); ok {
		// 결과는 같은 MQTT 클라이언트로 돌아옵니다. 여기서 기다리면
		// 메시지 처리가 막혀 결과를 영영 받지 못합니다.
		go a.execute(p, send)
	}
}

// sendAuto는 자동급수 명령을 보내고 결과를 읽습니다.
func sendAuto(client mqtt.Client, deviceID string, p *pending, dose int) (waterResult, error) {
	raw, err := sendWater(context.Background(), client, deviceID, p, newID("auto"), dose)
	if err != nil {
		return waterResult{}, err
	}
	var res waterResult
	if err := json.Unmarshal(raw, &res); err != nil {
		return waterResult{}, fmt.Errorf("결과를 읽지 못했습니다: %w", err)
	}
	return res, nil
}

type autoStatus struct {
	Enabled bool `json:"enabled"`
	DryPct  int  `json:"dry_pct"`
	DoseML  int  `json:"dose_ml"`
	// 줬는데 흙수분이 오르지 않았거나 하드웨어 오류로 멈췄습니다.
	Stopped bool `json:"stopped"`
	// 지금 주지 못하는 이유. 사유 코드입니다(water/result의 reason과 같음).
	Hold       string `json:"hold,omitempty"`
	LastSentAt int64  `json:"last_sent_at,omitempty"`
}

func (a *autoWaterer) status() autoStatus {
	a.mu.Lock()
	defer a.mu.Unlock()
	s := autoStatus{
		Enabled: a.enabled,
		DryPct:  a.dryPct,
		DoseML:  a.doseML,
		Stopped: a.stopped,
		Hold:    a.hold,
	}
	if !a.lastSentAt.IsZero() {
		s.LastSentAt = a.lastSentAt.Unix()
	}
	return s
}

// autoHandler는 자동급수 상태를 주고, 켜고 끕니다.
func autoHandler(a *autoWaterer) func(http.ResponseWriter, *http.Request) {
	return func(w http.ResponseWriter, r *http.Request) {
		switch r.Method {
		case http.MethodGet:
			writeJSON(w, http.StatusOK, a.status())
		case http.MethodPost:
			// 켜는 것은 물이 나가게 하는 일이라 급수와 같은 방어를 겁니다.
			if err := guardWrite(w, r); err != nil {
				return
			}
			// 같은 값의 enabled는 아무것도 바꾸지 않습니다. 멈춘 것을 다시
			// 맡기려면 resume을 따로 보냅니다.
			var req struct {
				Enabled *bool `json:"enabled"`
				Resume  bool  `json:"resume"`
			}
			if err := json.NewDecoder(http.MaxBytesReader(w, r.Body, 1<<10)).Decode(&req); err != nil || (req.Enabled == nil && !req.Resume) {
				writeJSON(w, http.StatusBadRequest, map[string]any{
					"error": `{"enabled": true|false} 또는 {"resume": true}로 보내야 합니다`,
				})
				return
			}
			var err error
			if req.Enabled != nil {
				err = a.SetEnabled(*req.Enabled)
				log.Printf("자동급수 켜짐 요청: %v → 지금 %v", *req.Enabled, a.Enabled())
			}
			if err == nil && req.Resume {
				err = a.Resume()
				log.Print("자동급수 다시 맡김")
			}
			if err != nil {
				log.Printf("자동급수 설정 저장 실패: %v", err)
				writeJSON(w, http.StatusInternalServerError, map[string]any{
					"error": "설정을 저장하지 못했습니다. 재시작하면 이전 값으로 돌아갑니다",
				})
				return
			}
			writeJSON(w, http.StatusOK, a.status())
		default:
			w.Header().Set("Allow", "GET, POST")
			writeJSON(w, http.StatusMethodNotAllowed, map[string]any{
				"error": "GET이나 POST만 받습니다",
			})
		}
	}
}
