//! BLE 스캔 태스크. 스펙 `docs/pilot-design.md` §4.4의 `ble_scan`.
//!
//! HHCC는 GATT 연결 없이 광고에 측정값을 실어 보냅니다. passive scan이라
//! 우리 쪽에서 요청을 보내지 않고, 상대 배터리를 축내지 않습니다.

use esp32_nimble::utilities::BleUuid;
use esp32_nimble::{BLEDevice, BLEScan};
use esp_idf_svc::hal::task::block_on;
use namo_core::mibeacon::{self, ParseError};

use crate::clock;
use crate::state::Shared;

/// Xiaomi MiBeacon 서비스 UUID.
const MIBEACON_UUID: u16 = 0xFE95;

/// 한 번의 스캔 구간. 끝나면 바로 다시 시작합니다.
const SCAN_MS: i32 = 10_000;

/// 광고를 계속 받아 공유 상태를 갱신합니다. 돌아오지 않습니다.
///
/// 측정값에 붙일 시각은 [`clock::now`]에서 가져옵니다. 시계가 아직 안 맞았으면
/// 값을 버립니다. 신선도를 판단할 수 없는 측정값은 텔레메트리에서
/// `sensor_seen_ago_s`를 거짓말하게 만들기 때문입니다.
pub fn run(shared: Shared) {
    log::info!("BLE 스캔 태스크 시작");
    let target = BleUuid::from_uuid16(MIBEACON_UUID);
    let mut last_seen: Option<([u8; 6], u8)> = None;

    block_on(async {
        let device = BLEDevice::take();
        let mut scan = BLEScan::new();
        scan.active_scan(false).interval(100).window(99);

        loop {
            let result = scan
                .start(device, SCAN_MS, |_device, data| {
                    let service = data.service_data()?;
                    if service.uuid != target {
                        return None::<()>;
                    }
                    handle(service.service_data, &shared, &mut last_seen);
                    None
                })
                .await;

            if let Err(e) = result {
                log::error!("BLE 스캔 실패: {e:?}");
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
        }
    });
}

fn handle(raw: &[u8], shared: &Shared, last_seen: &mut Option<([u8; 6], u8)>) {
    let (header, measurements) = match mibeacon::parse(raw) {
        Ok(v) => v,
        // HHCC가 아닌 Xiaomi 기기도 같은 UUID를 씁니다. 흔한 상황입니다.
        Err(ParseError::NotHhcc) => return,
        Err(ParseError::Encrypted) => {
            log::warn!("암호화된 MiBeacon 광고입니다. 앱에서 바인딩을 풀어야 합니다.");
            return;
        }
        Err(_) => return,
    };

    // 같은 광고의 재전송은 건너뜁니다.
    if *last_seen == Some((header.mac, header.frame_counter)) {
        return;
    }
    *last_seen = Some((header.mac, header.frame_counter));

    let Some(now) = clock::now() else {
        return;
    };

    if let Ok(mut state) = shared.lock() {
        state.sensor.apply_all(measurements.as_slice(), now);
    }
}
