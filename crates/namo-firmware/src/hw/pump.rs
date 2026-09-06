//! 펌프 드라이버. 스펙 `docs/pilot-design.md` §4.5의 S1·S3a·S11.
//!
//! 이 모듈의 목적은 "상위 로직이 아무리 틀려도 펌프는 반드시 꺼진다"를
//! 보장하는 것입니다. 그래서 최대 구동시간을 인자가 아니라 드라이버 내부
//! 상수로 두고, 호출자가 넘긴 값이 그보다 크면 조용히 잘라냅니다.

use esp_idf_svc::hal::gpio::{AnyOutputPin, Output, PinDriver};
use esp_idf_svc::sys::EspError;
use namo_core::safety::AbortReason;
use std::time::{Duration, Instant};

use crate::config::{ABORT_CHECK_MS, MAX_PUMP_MS};

/// 한 번의 구동이 어떻게 끝났는지.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct PumpOutcome {
    /// 실제로 펌프가 돈 시간. 추정 급수량은 이 값으로 계산합니다.
    pub actual_ms: u32,
    /// 중단되었다면 그 사유. `None`이면 계획한 시간을 다 채웠습니다.
    pub aborted: Option<AbortReason>,
    /// 요청이 하드리밋에 걸려 잘렸는지. 로그로 남겨 보정값 오류를 알아챕니다.
    pub hit_hard_limit: bool,
}

/// 펌프 GPIO를 독점하는 드라이버.
///
/// 이 타입을 만드는 순간 핀이 출력 LOW로 확정됩니다(S1). 소유권이 하나뿐이라
/// 다른 코드가 같은 핀을 건드릴 수 없습니다.
pub struct Pump<'d> {
    pin: PinDriver<'d, Output>,
}

impl<'d> Pump<'d> {
    /// 핀을 출력으로 잡고 즉시 LOW로 내립니다.
    ///
    /// `main`에서 가장 먼저 호출해야 합니다. WiFi·BLE 초기화보다 앞서야
    /// 하며, 그 사이에 어떤 실패가 나도 펌프가 돌지 않습니다.
    pub fn new(pin: AnyOutputPin<'d>) -> Result<Self, EspError> {
        let mut driver = PinDriver::output(pin)?;
        driver.set_low()?;
        Ok(Self { pin: driver })
    }

    /// 지금 펌프가 켜져 있는지.
    pub fn is_running(&self) -> bool {
        self.pin.is_set_high()
    }

    /// 펌프를 끕니다. 여러 번 불러도 안전합니다.
    pub fn stop(&mut self) -> Result<(), EspError> {
        self.pin.set_low()
    }

    /// 펌프를 돌립니다. 중단조건을 주기적으로 확인하며, 어떤 경로로 끝나든
    /// 반환 직전에 핀을 LOW로 되돌립니다.
    ///
    /// `requested_ms`가 [`MAX_PUMP_MS`]보다 크면 잘라냅니다(S3a). 호출자가
    /// 이 상한을 올릴 방법은 없습니다.
    ///
    /// `should_abort`는 [`ABORT_CHECK_MS`]마다 호출됩니다. `Some`을 돌려주면
    /// 즉시 멈춥니다(S4·S5). 이 콜백 안에서 네트워크 I/O를 하면 안 됩니다.
    /// 펌프가 도는 동안 블로킹되면 그만큼 물이 더 나갑니다.
    ///
    /// 시간은 [`Instant`]로만 잽니다. 네트워크나 SNTP와 무관하게 종료가
    /// 보장돼야 하기 때문입니다(S11).
    pub fn run<F>(&mut self, requested_ms: u32, mut should_abort: F) -> Result<PumpOutcome, EspError>
    where
        F: FnMut() -> Option<AbortReason>,
    {
        let hit_hard_limit = requested_ms > MAX_PUMP_MS;
        let target = Duration::from_millis(requested_ms.min(MAX_PUMP_MS) as u64);

        if hit_hard_limit {
            log::warn!(
                "펌프 요청 {requested_ms}ms가 하드리밋 {MAX_PUMP_MS}ms를 넘어 잘렸습니다. \
                 보정값이나 상위 계산을 의심하세요."
            );
        }

        let started = Instant::now();
        self.pin.set_high()?;

        let step = Duration::from_millis(ABORT_CHECK_MS as u64);
        let mut aborted = None;

        loop {
            let elapsed = started.elapsed();
            if elapsed >= target {
                break;
            }

            if let Some(reason) = should_abort() {
                aborted = Some(reason);
                break;
            }

            // 남은 시간이 검사 주기보다 짧으면 그만큼만 잡니다. 그래야 목표
            // 시간을 넘겨 도는 일이 없습니다.
            std::thread::sleep(step.min(target - elapsed));
        }

        // 성공·중단·에러 어느 경로로도 반드시 여기를 지납니다.
        self.pin.set_low()?;

        let actual_ms = started.elapsed().as_millis().min(u32::MAX as u128) as u32;

        Ok(PumpOutcome {
            actual_ms,
            aborted,
            hit_hard_limit,
        })
    }
}

impl Drop for Pump<'_> {
    fn drop(&mut self) {
        // 드라이버가 사라질 때 핀이 HIGH로 남으면 펌프가 계속 돕니다.
        // 여기서 실패해도 할 수 있는 일이 없으므로 로그만 남깁니다.
        if let Err(e) = self.pin.set_low() {
            log::error!("펌프 핀을 내리지 못했습니다: {e}");
        }
    }
}
