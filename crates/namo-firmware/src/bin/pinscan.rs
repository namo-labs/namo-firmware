//! bring-up 보조 도구. 보드 실크스크린의 핀 라벨(`D4`, `A2` 등)이 실제 GPIO
//! 번호와 어떻게 대응하는지 실측합니다.
//!
//! 소형 ESP32-S3 보드는 GPIO 번호 대신 `D0`~`D10` 같은 자체 라벨을 찍는
//! 경우가 많고, 그 대응은 보드마다 다릅니다. 데이터시트를 찾기 어렵거나
//! 못 믿을 때 이 도구로 확정합니다.
//!
//! 사용법: 플래싱한 뒤 점퍼선 한쪽을 GND에, 다른 쪽을 확인할 핀에 댑니다.
//! 닿는 순간 해당 GPIO 번호가 로그에 찍힙니다.

use esp_idf_svc::hal::gpio::{AnyIOPin, PinDriver, Pull};
use std::thread::sleep;
use std::time::Duration;

/// 스캔 대상 GPIO.
///
/// 제외한 핀과 이유:
/// - 0: BOOT 버튼에 연결돼 항상 눌림/떼임으로 읽혀 혼동을 줍니다.
/// - 19·20: USB D-/D+. 시리얼 로그가 이 핀으로 나가므로 건드리면 통신이 끊깁니다.
/// - 26~32: 내장 SPI 플래시. 건드리면 즉시 크래시합니다.
///
/// 33~37은 옥탈 PSRAM 보드에서만 위험한데, 이 보드는 PSRAM이 없어 안전합니다
/// (`hardware-pilot-status.md` H2).
const CANDIDATES: &[u8] = &[
    1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 21, 33, 34, 35, 36, 37, 38, 39,
    40, 41, 42, 43, 44, 45, 46, 47, 48,
];

fn main() {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    let mut pins = Vec::new();
    let mut skipped = Vec::new();

    for &n in CANDIDATES {
        // SAFETY: CANDIDATES에 중복이 없으므로 각 GPIO를 한 번씩만 가져옵니다.
        let pin = unsafe { AnyIOPin::steal(n) };
        match PinDriver::input(pin, Pull::Up) {
            Ok(driver) => pins.push((n, driver)),
            Err(_) => skipped.push(n),
        }
    }

    log::info!("=== 핀 스캐너 ===");
    log::info!("감시 중인 GPIO {}개", pins.len());
    if !skipped.is_empty() {
        log::warn!("초기화 실패해 제외한 GPIO: {skipped:?}");
    }
    log::info!("점퍼선 한쪽을 GND에 꽂고, 다른 쪽 끝을 확인할 핀에 대세요.");
    log::info!("닿으면 GPIO 번호가 찍힙니다. 손을 떼면 해제 로그가 찍힙니다.");

    // 내부 풀업이 걸려 있으므로 평상시 HIGH, GND에 닿으면 LOW입니다.
    let mut was_low = vec![false; pins.len()];

    loop {
        for (index, (gpio, driver)) in pins.iter().enumerate() {
            let is_low = driver.is_low();
            if is_low != was_low[index] {
                if is_low {
                    log::info!("★ GPIO{gpio} ← GND에 닿음");
                } else {
                    log::info!("  GPIO{gpio} 해제");
                }
                was_low[index] = is_low;
            }
        }
        sleep(Duration::from_millis(30));
    }
}
