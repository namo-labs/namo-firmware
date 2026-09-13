//! 태스크들이 함께 보는 상태. 스펙 `docs/pilot-design.md` §4.4.
//!
//! 락은 이 하나뿐입니다. **락을 잡은 채로 네트워크 I/O나 펌프 구동을 하지
//! 않습니다.** 펌프가 도는 20초 동안 락이 잡혀 있으면 텔레메트리도 BLE도
//! 멈춥니다. 필요한 값만 꺼내 복사하고 즉시 놓습니다.

use namo_core::command::RecentIds;
use namo_core::daily::{DailyTotal, KST_OFFSET_S};
use namo_core::safety::{self, Leak, LeakSensor, Reservoir, SafetyInput};
use namo_core::telemetry::SensorState;
use std::sync::{Arc, Mutex};

/// 펌프의 현재 상태. 텔레메트리에 그대로 실립니다.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum PumpState {
    Idle,
    Running,
}

impl PumpState {
    pub fn as_str(&self) -> &'static str {
        match self {
            PumpState::Idle => "idle",
            PumpState::Running => "running",
        }
    }
}

#[derive(Debug)]
pub struct SharedState {
    pub sensor: SensorState,
    pub reservoir: Reservoir,
    pub leak_tank: LeakSensor,
    pub leak_pot: LeakSensor,
    /// 누수 센서를 날라주는 게이트웨이(Zigbee2MQTT)의 생존 여부.
    /// `None`이면 아직 소식을 못 들은 상태입니다.
    pub gateway_online: Option<bool>,
    /// 누수로 잠긴 상태(S5). `unlock` 명령으로만 풀립니다.
    pub locked: bool,
    pub pump: PumpState,
    pub last_completed_at: Option<u64>,
    pub daily: DailyTotal,
    pub recent_ids: RecentIds,
}

impl Default for SharedState {
    fn default() -> Self {
        Self::new()
    }
}

impl SharedState {
    pub fn new() -> Self {
        Self {
            sensor: SensorState::new(),
            // 부팅 직후에는 플로트를 아직 못 읽었습니다. 모르는 상태에서
            // 급수를 거부하도록 Unknown으로 시작합니다(fail-closed).
            reservoir: Reservoir::Unknown,
            leak_tank: LeakSensor::default(),
            leak_pot: LeakSensor::default(),
            gateway_online: None,
            locked: false,
            pump: PumpState::Idle,
            last_completed_at: None,
            daily: DailyTotal::new(),
            recent_ids: RecentIds::new(),
        }
    }

    /// 두 누수 센서를 합친 상태. 판정은 `namo-core`에 있습니다.
    ///
    /// 게이트웨이가 죽었으면 센서 값이 아무리 최근이어도 믿지 않습니다.
    pub fn leak(&self) -> Leak {
        safety::combine_leak(self.gateway_online, &[self.leak_tank, self.leak_pot])
    }

    /// 누수 정보의 신선도를 판단할 기준 시각.
    pub fn leak_updated_at(&self) -> Option<u64> {
        safety::oldest_update(&[self.leak_tank, self.leak_pot])
    }

    /// 안전 판정에 넘길 입력을 만듭니다.
    ///
    /// `now`가 `None`이면 아직 시각 동기화가 안 된 것이고, 판정은
    /// `clock_unsynced`로 거부합니다(fail-closed).
    pub fn safety_input(&self, now: Option<u64>, dose_ml: u32) -> SafetyInput {
        SafetyInput {
            now,
            reservoir: self.reservoir,
            leak: self.leak(),
            leak_updated_at: self.leak_updated_at(),
            locked: self.locked,
            running: self.pump == PumpState::Running,
            last_completed_at: self.last_completed_at,
            today_ml: now.map_or(0, |t| self.daily.today_ml(t, KST_OFFSET_S)),
            dose_ml,
        }
    }
}

/// 태스크 간에 넘기는 핸들.
pub type Shared = Arc<Mutex<SharedState>>;

pub fn new_shared() -> Shared {
    Arc::new(Mutex::new(SharedState::new()))
}
