//! bring-up 보조 도구. MOSFET이 3.3V 게이트 신호로 완전히 켜지는지
//! 확인합니다 (`docs/bring-up-guide.md` Stage 2).
//!
//! **펌프를 연결하지 않은 상태에서 돌립니다.** 출력 양단 전압만 재는 것이
//! 목적이며, 여기서 부분 도통을 못 걸러내면 다음 단계에서 펌프까지 상합니다.
//!
//! 실제 `Pump` 드라이버를 그대로 씁니다. 하드리밋과 종료 보장이 본 펌웨어와
//! 같은 코드로 동작하는지 함께 확인하기 위함입니다.

use esp_idf_svc::hal::gpio::AnyOutputPin;
use namo_firmware::config::PIN_PUMP;
use namo_firmware::hw::pump::Pump;
use std::thread::sleep;
use std::time::Duration;

/// 한 번 켜두는 시간. 멀티미터를 읽을 여유를 줍니다.
const ON_MS: u32 = 6_000;
/// 끄고 기다리는 시간.
const OFF_S: u64 = 6;

fn main() {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    // SAFETY: 이 도구는 이 핀 하나만 사용합니다.
    let pin = unsafe { AnyOutputPin::steal(PIN_PUMP) };
    let mut pump = Pump::new(pin).expect("펌프 핀 초기화 실패");

    log::info!("=== MOSFET 무부하 테스트 ===");
    log::info!("GPIO{PIN_PUMP} (보드 라벨 D3)를 {}초씩 켜고 끕니다.", ON_MS / 1000);
    log::warn!("펌프가 연결돼 있으면 지금 뽑으세요. 무부하 상태에서만 의미가 있습니다.");
    log::info!("멀티미터 DC 전압 모드. 빨강 프로브 → +, 검정 프로브 → LOAD");
    log::info!("켜짐: 11.5V 이상이어야 합니다. 꺼짐: 0V 근처여야 합니다.");
    log::info!("");

    // 처음 한 번은 꺼진 상태를 충분히 보여줍니다.
    log::info!("◻︎ 꺼짐 — 지금 전압을 재세요 ({OFF_S}초)");
    sleep(Duration::from_secs(OFF_S));

    let mut round = 0u32;
    loop {
        round += 1;

        log::info!("◼︎ [{round}회차] 켜짐 — 지금 전압을 재세요");
        match pump.run(ON_MS, || None) {
            Ok(outcome) => log::info!(
                "   {}ms 구동 후 껐습니다 (하드리밋 걸림: {})",
                outcome.actual_ms,
                outcome.hit_hard_limit
            ),
            Err(e) => {
                log::error!("펌프 구동 실패: {e}");
                return;
            }
        }

        log::info!("◻︎ [{round}회차] 꺼짐 — 지금 전압을 재세요");
        sleep(Duration::from_secs(OFF_S));
    }
}
