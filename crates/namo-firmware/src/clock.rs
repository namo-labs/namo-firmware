//! 시각. 스펙 `docs/pilot-design.md` §4.5의 S8.
//!
//! 명령 TTL을 판단하려면 시계가 맞아야 합니다. 맞지 않은 시계로 TTL을 재면
//! 만료된 명령을 실행하거나 멀쩡한 명령을 버립니다. 그래서 동기화 여부를
//! 값 자체로 표현하고, 모르면 급수를 거부합니다(fail-closed).

use esp_idf_svc::sntp::{EspSntp, SntpConf, SyncStatus};
use esp_idf_svc::sys::EspError;
use std::time::{SystemTime, UNIX_EPOCH};

/// 이 시각보다 과거면 동기화가 안 된 것으로 봅니다.
///
/// 2025-01-01. ESP32는 전원을 켤 때마다 1970년부터 시작하므로, 이보다 앞선
/// 시각은 SNTP 응답을 받지 못했다는 뜻입니다.
const SYNCED_THRESHOLD: u64 = 1_735_689_600;

/// 현재 epoch 초. 시계를 신뢰할 수 없으면 `None`입니다.
pub fn now() -> Option<u64> {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    (secs >= SYNCED_THRESHOLD).then_some(secs)
}

/// SNTP를 시작합니다.
///
/// 반환된 핸들을 살려둬야 주기적 재동기화가 계속됩니다. 떨어뜨리면 동기화가
/// 멈추므로 `main`이 끝까지 들고 있어야 합니다.
pub fn start_sntp() -> Result<EspSntp<'static>, EspError> {
    EspSntp::new(&SntpConf::default())
}

/// 동기화가 끝날 때까지 최대 `timeout_s`초 기다립니다.
///
/// 실패해도 부팅은 계속합니다. 시각을 모르는 상태로도 센서 수집과 텔레메트리는
/// 의미가 있고, 급수만 거부하면 됩니다.
pub fn wait_synced(sntp: &EspSntp<'static>, timeout_s: u64) -> bool {
    for _ in 0..timeout_s {
        if sntp.get_sync_status() == SyncStatus::Completed && now().is_some() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
    false
}
