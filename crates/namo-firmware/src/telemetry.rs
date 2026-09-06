//! 텔레메트리 발행. 스펙 `docs/pilot-design.md` §5.2.

use namo_core::daily::KST_OFFSET_S;
use namo_core::safety::{self, Leak, Reservoir};
use serde::Serialize;
use std::sync::Arc;
use std::time::Duration;

use crate::clock;
use crate::config::{LIMITS, TELEMETRY_PERIOD_S};
use crate::net::mqtt::{Publisher, Topics};
use crate::state::Shared;

/// 한 시점의 상태. MQTT와 HTTP가 같은 것을 보여주도록 공유합니다.
#[derive(Debug, Serialize)]
pub struct Telemetry {
    pub ts: Option<u64>,
    pub soil_moisture_pct: Option<u8>,
    pub soil_temperature_c: Option<f32>,
    pub illuminance_lux: Option<u32>,
    pub soil_conductivity_us_cm: Option<u16>,
    pub sensor_seen_ago_s: Option<u64>,
    pub reservoir: &'static str,
    pub leak: &'static str,
    pub leak_seen_ago_s: Option<u64>,
    pub pump: &'static str,
    pub today_estimated_ml: u32,
    pub cooldown_until: Option<u64>,
    pub locked: bool,
}

fn reservoir_str(r: Reservoir) -> &'static str {
    match r {
        Reservoir::Ok => "ok",
        Reservoir::Empty => "empty",
        Reservoir::Unknown => "unknown",
    }
}

fn leak_str(l: Leak) -> &'static str {
    match l {
        Leak::None => "none",
        Leak::Detected => "detected",
        Leak::Unknown => "unknown",
    }
}

/// 현재 상태를 한 장 찍습니다.
///
/// 락을 잡은 채로 발행하지 않도록, 필요한 값만 복사해 즉시 놓습니다.
/// 상태 락이 깨졌으면 `None`입니다.
pub fn snapshot(shared: &Shared) -> Option<Telemetry> {
    let now = clock::now();
    let state = shared.lock().ok()?;

    // 쿨다운이 언제 풀리는지는 안전 판정과 같은 계산을 써야 텔레메트리와
    // 실제 동작이 어긋나지 않습니다.
    let input = state.safety_input(now, 0);
    let cooldown_until = safety::next_available_at(&input, &LIMITS);

    Some(Telemetry {
        ts: now,
        soil_moisture_pct: state.sensor.moisture_pct,
        soil_temperature_c: state
            .sensor
            .temperature_deci_c
            .map(|v| f32::from(v) / 10.0),
        illuminance_lux: state.sensor.illuminance_lux,
        soil_conductivity_us_cm: state.sensor.conductivity_us_cm,
        sensor_seen_ago_s: now.and_then(|t| state.sensor.age_s(t)),
        reservoir: reservoir_str(state.reservoir),
        leak: leak_str(state.leak()),
        leak_seen_ago_s: now
            .zip(state.leak_updated_at())
            .map(|(t, seen)| t.saturating_sub(seen)),
        pump: state.pump.as_str(),
        today_estimated_ml: now.map_or(0, |t| state.daily.today_ml(t, KST_OFFSET_S)),
        cooldown_until,
        locked: state.locked,
    })
}

/// 주기적으로 현재 상태를 발행합니다. 돌아오지 않습니다.
pub fn run(shared: Shared, publisher: Publisher, topics: Arc<Topics>) {
    loop {
        match snapshot(&shared) {
            Some(payload) => match serde_json::to_vec(&payload) {
                Ok(bytes) => publisher.publish(&topics.telemetry, &bytes, false),
                Err(e) => log::error!("텔레메트리 직렬화 실패: {e}"),
            },
            None => log::error!("상태 락이 깨졌습니다"),
        }

        std::thread::sleep(Duration::from_secs(TELEMETRY_PERIOD_S));
    }
}
