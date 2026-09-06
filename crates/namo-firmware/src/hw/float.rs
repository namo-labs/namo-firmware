//! 플로트 스위치 읽기. 판정 로직은 `namo-core`의 디바운서에 있습니다.

use esp_idf_svc::hal::gpio::{AnyIOPin, Input, PinDriver, Pull};
use esp_idf_svc::sys::EspError;
use namo_core::float::Debouncer;
use namo_core::safety::Reservoir;

/// 플로트 스위치 입력과 디바운서를 묶은 것.
pub struct FloatSwitch<'d> {
    pin: PinDriver<'d, Input>,
    debouncer: Debouncer,
}

impl<'d> FloatSwitch<'d> {
    /// 내부 풀업을 켜고 입력으로 잡습니다. 반대편은 GND에 연결돼 있어야 합니다.
    pub fn new(pin: AnyIOPin<'d>) -> Result<Self, EspError> {
        Ok(Self {
            pin: PinDriver::input(pin, Pull::Up)?,
            debouncer: Debouncer::default(),
        })
    }

    /// 한 번 읽고 디바운스를 거친 상태를 돌려줍니다.
    ///
    /// 플로트가 뜨면 접점이 닫혀 LOW이고, 그것이 "물 있음"입니다. 선이 빠지면
    /// 개방과 구별되지 않아 HIGH가 되고 `Empty`로 읽힙니다. 급수를 막는
    /// 방향이라 안전합니다.
    pub fn poll(&mut self) -> Reservoir {
        self.debouncer.update(self.pin.is_low())
    }

    /// 새로 읽지 않고 마지막으로 확정된 상태만 봅니다.
    pub fn state(&self) -> Reservoir {
        self.debouncer.state()
    }
}
