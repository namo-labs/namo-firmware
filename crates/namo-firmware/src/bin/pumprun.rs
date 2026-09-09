//! bring-up 보조 도구. 펌프를 정해진 시간만큼 **한 번만** 돌립니다
//! (`docs/bring-up-guide.md` Stage 3).
//!
//! 반복하지 않는 것이 핵심입니다. 첫 구동에서는 물이 어디로 얼마나 나가는지
//! 모르므로, 한 번 돌리고 멈춰서 관찰할 시간을 줍니다.
//!
//! 수중 펌프는 공회전하면 몇 초로도 상합니다. 반드시 물에 잠긴 상태에서만
//! 실행합니다.

use esp_idf_svc::hal::gpio::AnyOutputPin;
use namo_firmware::config::PIN_PUMP;
use namo_firmware::hw::pump::Pump;
use std::thread::sleep;
use std::time::Duration;

/// 구동 시간. Stage 3은 2초로 시작합니다.
const RUN_MS: u32 = 2_000;
/// 플래싱 직후 바로 돌면 준비할 틈이 없으므로 기다립니다.
///
/// 12V는 플래싱이 끝난 뒤에 연결해야 합니다. 부팅 중에는 GPIO가 잠깐 뜨는
/// 구간이 있어 펌프가 순간적으로 돌 수 있기 때문입니다. 그 순서를 지킬
/// 여유를 주려면 넉넉해야 합니다.
const COUNTDOWN_S: u64 = 30;

fn main() {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    // SAFETY: 이 도구는 이 핀 하나만 사용합니다.
    let pin = unsafe { AnyOutputPin::steal(PIN_PUMP) };
    let mut pump = Pump::new(pin).expect("펌프 핀 초기화 실패");

    log::info!("=== 펌프 첫 구동 ===");
    log::warn!("펌프가 물에 완전히 잠겨 있는지 확인하세요. 공회전은 펌프를 상하게 합니다.");
    log::warn!("튜브 끝이 빈 양동이 안에 있는지 확인하세요.");
    log::info!("{}초 뒤에 {}초간 한 번만 돌립니다.", COUNTDOWN_S, RUN_MS / 1000);
    log::warn!("지금 12V 어댑터를 콘센트에 꽂으세요.");

    for remaining in (1..=COUNTDOWN_S).rev() {
        if remaining % 5 == 0 || remaining <= 5 {
            log::info!("  {remaining}...");
        }
        sleep(Duration::from_secs(1));
    }

    log::info!("◼︎ 펌프 켬");
    match pump.run(RUN_MS, || None) {
        Ok(outcome) => {
            log::info!("◻︎ 펌프 끔 ({}ms 구동)", outcome.actual_ms);
            log::info!("");
            log::info!("확인할 것:");
            log::info!("  1. 물이 나왔는가");
            log::info!("  2. 지금 물이 **완전히** 멈췄는가 (계속 흐르면 사이펀입니다)");
            log::info!("  3. MOSFET 모듈이 미지근한 정도인가 (뜨거우면 전원을 뽑으세요)");
        }
        Err(e) => log::error!("펌프 구동 실패: {e}"),
    }

    // 다시 돌지 않습니다. 관찰이 끝나면 전원을 뽑거나 재부팅하세요.
    log::info!("끝났습니다. 다시 돌리려면 보드를 리셋하세요.");
    loop {
        sleep(Duration::from_secs(60));
    }
}
