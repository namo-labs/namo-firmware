package main

import (
	"fmt"
	"strings"
	"time"
)

// 경보 이름. 같은 이름은 한 번만 올라가고 한 번만 내려갑니다.
const (
	alertLeak      = "leak"
	alertLocked    = "locked"
	alertReservoir = "reservoir"
	alertDry       = "dry"
	alertDevice    = "device"
	alertGateway   = "gateway"
)

// alertRules는 텔레메트리를 보고 알릴 것을 판단합니다.
//
// 무엇을 알리지 **않을지**가 더 중요합니다. 흙수분이 임계값 근처에서
// 오르내릴 때마다 보내면 하루에 수십 통이 됩니다. 그래서 경보를 내릴
// 때는 임계값보다 넉넉히 회복해야 하고(히스테리시스), 장치나 게이트웨이가
// 잠깐 끊긴 것은 일정 시간을 두고 봅니다.
type alertRules struct {
	n *Notifier

	dryPct int
	// 임계값을 다시 넘었다고 바로 경보를 내리면, 값이 경계에서 흔들릴 때
	// 알림이 반복됩니다. 이만큼 더 올라와야 내립니다.
	dryClearMargin int

	// 장치가 조용해도 이 시간까지는 기다립니다. WiFi가 잠깐 끊기거나
	// 재부팅하는 동안 알림이 가면 성가십니다.
	deviceGrace  time.Duration
	gatewayGrace time.Duration

	deviceQuietSince time.Time
	gatewayDownSince time.Time
}

func newAlertRules(n *Notifier, dryPct int) *alertRules {
	return &alertRules{
		n:              n,
		dryPct:         dryPct,
		dryClearMargin: 10,
		deviceGrace:    5 * time.Minute,
		gatewayGrace:   10 * time.Minute,
	}
}

// alertState는 판정에 필요한 필드만 봅니다.
type alertState struct {
	Moisture  *int    `json:"soil_moisture_pct"`
	Reservoir *string `json:"reservoir"`
	Leak      *string `json:"leak"`
	Locked    *bool   `json:"locked"`
	Gateway   *bool   `json:"gateway_online"`
	Today     *int    `json:"today_estimated_ml"`
}

// check는 한 번의 텔레메트리를 보고 필요한 알림을 보냅니다.
func (r *alertRules) check(s alertState, now time.Time) {
	if r.n == nil {
		return
	}

	// 장치가 살아 있다는 뜻이므로 조용함 경보를 내립니다.
	r.deviceQuietSince = time.Time{}
	r.n.Clear(alertDevice, "✅ <b>장치가 돌아왔습니다</b>")

	// ── 누수: 가장 급합니다 ────────────────────────────────────
	if s.Leak != nil && *s.Leak == "detected" {
		r.n.Raise(alertLeak, strings.Join([]string{
			"🚨 <b>누수 감지</b>",
			"",
			"물이 새고 있습니다. 급수가 즉시 중단되고 잠겼습니다.",
			"물통과 화분 주변을 확인하세요.",
		}, "\n"))
	} else if s.Leak != nil && *s.Leak == "none" {
		r.n.Clear(alertLeak, "✅ <b>누수가 해소됐습니다</b>\n\n잠금은 그대로입니다. 확인 후 풀어주세요.")
	}

	// ── 잠김: 사람이 풀기 전에는 급수가 안 됩니다 ──────────────
	if s.Locked != nil && *s.Locked {
		r.n.Raise(alertLocked, strings.Join([]string{
			"🔒 <b>급수가 잠겼습니다</b>",
			"",
			"누수가 감지되어 잠겼습니다. 원인을 확인한 뒤",
			"화면에서 잠금을 풀어야 급수가 재개됩니다.",
		}, "\n"))
	} else if s.Locked != nil {
		r.n.Clear(alertLocked, "✅ <b>잠금이 풀렸습니다</b>")
	}

	// ── 물통 ───────────────────────────────────────────────────
	if s.Reservoir != nil && *s.Reservoir == "empty" {
		r.n.Raise(alertReservoir, strings.Join([]string{
			"⚠️ <b>물통이 비었습니다</b>",
			"",
			"물을 채우기 전까지 급수가 거부됩니다.",
		}, "\n"))
	} else if s.Reservoir != nil && *s.Reservoir == "ok" {
		r.n.Clear(alertReservoir, "✅ <b>물통이 채워졌습니다</b>")
	}

	// ── 흙이 마름 ──────────────────────────────────────────────
	//
	// 임계값을 내려갈 때 올리고, 넉넉히 회복해야 내립니다. 경계에서
	// 오르내릴 때 알림이 반복되지 않게 하기 위한 것입니다.
	if s.Moisture != nil {
		m := *s.Moisture
		switch {
		case m < r.dryPct:
			today := 0
			if s.Today != nil {
				today = *s.Today
			}
			r.n.Raise(alertDry, strings.Join([]string{
				fmt.Sprintf("💧 <b>흙이 말랐습니다 — %d%%</b>", m),
				"",
				fmt.Sprintf("기준 %d%% 아래로 내려갔습니다.", r.dryPct),
				fmt.Sprintf("오늘 급수: %dmL / 500mL", today),
				"",
				"화면에서 물을 줄 수 있습니다.",
			}, "\n"))
		case m >= r.dryPct+r.dryClearMargin:
			r.n.Clear(alertDry, fmt.Sprintf("✅ <b>흙이 젖었습니다 — %d%%</b>", m))
		}
	}

	// ── 게이트웨이 ─────────────────────────────────────────────
	//
	// 죽으면 누수를 알 수 없어 급수가 통째로 거부됩니다. 다만 스스로
	// 되살아나므로, 잠깐 끊긴 것까지 알리지는 않습니다.
	if s.Gateway != nil && !*s.Gateway {
		if r.gatewayDownSince.IsZero() {
			r.gatewayDownSince = now
		} else if now.Sub(r.gatewayDownSince) > r.gatewayGrace {
			r.n.Raise(alertGateway, strings.Join([]string{
				"⚠️ <b>Zigbee 게이트웨이가 끊겼습니다</b>",
				"",
				fmt.Sprintf("%s째 복구되지 않고 있습니다.", humanDur(now.Sub(r.gatewayDownSince))),
				"누수를 감지할 수 없어 급수가 거부됩니다.",
				"",
				"동글이 빠졌는지 확인하세요.",
			}, "\n"))
		}
	} else if s.Gateway != nil {
		r.gatewayDownSince = time.Time{}
		r.n.Clear(alertGateway, "✅ <b>게이트웨이가 돌아왔습니다</b>")
	}
}

// checkQuiet은 텔레메트리가 한동안 오지 않을 때 부릅니다.
func (r *alertRules) checkQuiet(lastSeen time.Time, now time.Time) {
	if r.n == nil || lastSeen.IsZero() {
		return
	}
	quiet := now.Sub(lastSeen)
	if quiet < r.deviceGrace {
		return
	}
	if r.deviceQuietSince.IsZero() {
		r.deviceQuietSince = lastSeen
	}
	r.n.Raise(alertDevice, strings.Join([]string{
		"⚠️ <b>장치가 응답하지 않습니다</b>",
		"",
		fmt.Sprintf("%s째 텔레메트리가 오지 않습니다.", humanDur(quiet)),
		"전원이나 WiFi를 확인하세요.",
	}, "\n"))
}
