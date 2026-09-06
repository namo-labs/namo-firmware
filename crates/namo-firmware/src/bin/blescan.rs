//! bring-up 보조 도구. HHCC Flower Care의 BLE 광고를 잡아 `namo-core`의
//! MiBeacon 파서에 물립니다 (`docs/bring-up-guide.md` Stage 5).
//!
//! GATT 연결 없이 passive scan만 씁니다. HHCC는 측정값을 광고에 실어
//! 보내므로 연결이 필요 없고, 연결하지 않는 편이 배터리와 안정성 모두
//! 유리합니다. 한 광고에는 측정값이 하나만 들어오므로 네 값을 모두 보려면
//! 여러 광고를 모아야 합니다.

use esp32_nimble::utilities::BleUuid;
use esp32_nimble::{BLEDevice, BLEScan};
use esp_idf_svc::hal::task::block_on;
use namo_core::mibeacon::{self, Measurement, ParseError};

/// Xiaomi MiBeacon 서비스 UUID.
const MIBEACON_UUID: u16 = 0xFE95;

/// 한 번의 스캔 구간. 끝나면 다시 시작해 무한히 돕니다.
const SCAN_MS: i32 = 10_000;

fn main() {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    log::info!("=== BLE 스캐너 (MiBeacon) ===");
    log::info!("서비스 UUID 0x{MIBEACON_UUID:04X} 광고만 골라 봅니다.");
    log::info!("HHCC를 보드 근처에 두세요. 광고는 보통 몇 초에 한 번 나옵니다.");

    let target = BleUuid::from_uuid16(MIBEACON_UUID);

    // 같은 광고가 여러 번 잡히면 로그가 폭주합니다. MiBeacon은 프레임 카운터를
    // 실어 보내므로, 직전과 (MAC, 카운터)가 같으면 재전송으로 보고 건너뜁니다.
    let mut last_seen: Option<([u8; 6], u8)> = None;

    block_on(async {
        let device = BLEDevice::take();
        let mut scan = BLEScan::new();
        // passive scan. scan request를 보내지 않으므로 상대 배터리를 아낍니다.
        scan.active_scan(false).interval(100).window(99);

        loop {
            let result = scan
                .start(device, SCAN_MS, |_device, data| {
                    let service = data.service_data()?;
                    if service.uuid != target {
                        return None::<()>;
                    }
                    handle(service.service_data, &mut last_seen);
                    None
                })
                .await;

            if let Err(e) = result {
                log::error!("스캔 실패: {e:?}");
            }
        }
    });
}

fn handle(raw: &[u8], last_seen: &mut Option<([u8; 6], u8)>) {
    match mibeacon::parse(raw) {
        Ok((header, measurements)) => {
            if *last_seen == Some((header.mac, header.frame_counter)) {
                return;
            }
            *last_seen = Some((header.mac, header.frame_counter));

            let m = header.mac;
            log::info!(
                "── HHCC {:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X} (type 0x{:04X}, #{})",
                m[0],
                m[1],
                m[2],
                m[3],
                m[4],
                m[5],
                header.device_type,
                header.frame_counter
            );

            if measurements.is_empty() {
                log::info!("   측정값 없는 광고 (기기가 살아있다는 신호)");
            }
            for item in measurements.as_slice() {
                match item {
                    Measurement::TemperatureDeciC(v) => {
                        log::info!("   온도    {}.{}℃", v / 10, (v % 10).abs());
                    }
                    Measurement::MoisturePct(v) => log::info!("   흙수분  {v}%"),
                    Measurement::IlluminanceLux(v) => log::info!("   조도    {v} lux"),
                    Measurement::ConductivityUsCm(v) => log::info!("   전도도  {v} µS/cm"),
                    Measurement::BatteryPct(v) => log::info!("   배터리  {v}%"),
                }
            }
        }
        // HHCC가 아닌 Xiaomi 기기(온습도계 등)도 같은 UUID를 씁니다. 흔한
        // 상황이라 조용히 넘깁니다.
        Err(ParseError::NotHhcc) => {}
        // 바인딩된 기기는 페이로드를 암호화합니다. 이 경우 passive scan으로는
        // 값을 볼 수 없으므로 알려줍니다.
        Err(ParseError::Encrypted) => {
            log::warn!("암호화된 MiBeacon 광고입니다. 앱에서 바인딩을 풀어야 합니다.");
        }
        Err(_) => {}
    }
}
