//! bring-up 보조 도구. 플로트 스위치가 물통의 물 유무를 제대로 읽는지
//! 확인합니다 (`docs/bring-up-guide.md` Stage 1-D).
//!
//! 판정 로직은 `namo-core`의 디바운서를 그대로 씁니다. 여기서 본 동작이
//! 본 펌웨어의 동작과 같아야 의미가 있습니다.

use esp_idf_svc::hal::gpio::{AnyIOPin, PinDriver, Pull};
use namo_core::float::Debouncer;
use namo_core::safety::Reservoir;
use std::thread::sleep;
use std::time::Duration;

/// 플로트 스위치 입력. 보드 라벨로는 `D4`입니다 (`pilot-design.md` §3.1).
const FLOAT_GPIO: u8 = 5;

/// 폴링 주기. 디바운서 기본값 5회와 곱해 100ms가 됩니다.
const POLL: Duration = Duration::from_millis(20);

fn main() {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    // 펌프 GPIO(4)는 이 도구에서 건드리지 않습니다. 출력으로 잡지 않으므로
    // 부팅 내내 입력 상태로 남고, 외부 풀다운이 게이트를 LOW로 유지합니다.

    // SAFETY: 이 도구는 이 핀 하나만 사용하므로 드라이버가 중복되지 않습니다.
    let pin = unsafe { AnyIOPin::steal(FLOAT_GPIO) };
    let input = PinDriver::input(pin, Pull::Up).expect("GPIO 입력 설정 실패");

    let mut debouncer = Debouncer::default();
    let mut last = Reservoir::Unknown;

    log::info!("=== 플로트 스위치 테스트 ===");
    log::info!("GPIO{FLOAT_GPIO} (보드 라벨 D4) 감시 중");
    log::info!("플로트를 올리면 ok, 내리면 empty가 떠야 합니다.");
    log::info!("반대로 나오면 배선이나 논리가 뒤집힌 것이니 여기서 멈추세요.");

    loop {
        let state = debouncer.update(input.is_low());

        if state != last {
            match state {
                Reservoir::Ok => log::info!("★ reservoir = ok    (물 있음 / 플로트 뜸)"),
                Reservoir::Empty => log::warn!("★ reservoir = empty (물 없음 / 플로트 내려감)"),
                Reservoir::Unknown => log::warn!("★ reservoir = unknown"),
            }
            last = state;
        }

        sleep(POLL);
    }
}
