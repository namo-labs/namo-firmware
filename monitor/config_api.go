package main

import "net/http"

// 장치가 강제하는 한도들.
//
// **`namo-core`의 `Limits::DEFAULT`와 같은 값이어야 합니다.** 여기서
// 틀리면 화면과 앱이 거짓말을 하게 됩니다. 실제 판정은 언제나 장치가
// 하므로 이 값들은 미리 보여주기 위한 것입니다.
//
// 텔레메트리에 싣지 않고 여기 두는 이유는, 거의 바뀌지 않는 값을 10초마다
// 실어 보낼 이유가 없기 때문입니다. 장치 쪽을 고치면 이 파일도 함께
// 고칩니다.
const (
	deviceDailyLimitML = 500
	deviceMaxDoseML    = 300
	deviceCooldownS    = 1800
	deviceMaxPumpMS    = 10000
)

func configHandler(cfg config) func(http.ResponseWriter, *http.Request) {
	// 화면이 그릴 급수 버튼. allowedDoses는 맵이라 순서가 없으므로
	// 작은 것부터 정렬해 내보냅니다.
	doses := make([]int, 0, len(allowedDoses))
	for _, d := range []int{50, 100, 200, 300} {
		if allowedDoses[d] {
			doses = append(doses, d)
		}
	}

	return func(w http.ResponseWriter, r *http.Request) {
		writeJSON(w, http.StatusOK, map[string]any{
			"device_id": cfg.deviceID,
			"limits": map[string]any{
				"daily_limit_ml": deviceDailyLimitML,
				"max_dose_ml":    deviceMaxDoseML,
				"cooldown_s":     deviceCooldownS,
				"max_pump_ms":    deviceMaxPumpMS,
			},
			"allowed_doses": doses,
			"alerts": map[string]any{
				"dry_pct": cfg.dryPct,
				"enabled": cfg.telegramToken != "" && cfg.telegramChatID != "",
			},
			"history": map[string]any{
				"min_gap_s": cfg.minGapS,
				"max_age_s": cfg.maxAgeS,
			},
			"camera": map[string]any{
				// 카메라는 맥에서 도는 서버로 바로 가므로 이 서비스를
				// 거치지 않습니다. 경로만 알려줍니다.
				"hls":      "/namo/cam/stream.m3u8",
				"snapshot": "/namo/cam/snapshot.jpg",
			},
		})
	}
}
