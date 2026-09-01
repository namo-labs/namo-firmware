//! 광고로 들어오는 측정값을 누적해 현재 센서 상태를 만듭니다.

use crate::mibeacon::Measurement;

/// 여러 광고를 누적한 현재 센서 상태.
///
/// 한 번도 수집하지 못한 값은 `None`입니다. 임의의 기본값으로 채우지 않습니다.
#[derive(Debug, PartialEq, Eq, Clone, Copy, Default)]
pub struct SensorState {
    pub moisture_pct: Option<u8>,
    /// 0.1℃ 단위.
    pub temperature_deci_c: Option<i16>,
    pub illuminance_lux: Option<u32>,
    pub conductivity_us_cm: Option<u16>,
    pub battery_pct: Option<u8>,
    /// 마지막으로 어떤 값이든 받은 시각.
    pub last_seen: Option<u64>,
}

impl SensorState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn apply(&mut self, m: Measurement, now: u64) {
        match m {
            Measurement::MoisturePct(v) => self.moisture_pct = Some(v),
            Measurement::TemperatureDeciC(v) => self.temperature_deci_c = Some(v),
            Measurement::IlluminanceLux(v) => self.illuminance_lux = Some(v),
            Measurement::ConductivityUsCm(v) => self.conductivity_us_cm = Some(v),
            Measurement::BatteryPct(v) => self.battery_pct = Some(v),
        }
        self.last_seen = Some(now);
    }

    pub fn apply_all(&mut self, ms: &[Measurement], now: u64) {
        for m in ms {
            self.apply(*m, now);
        }
    }

    /// 마지막 수신 이후 경과 초. 아직 한 번도 못 받았으면 `None`입니다.
    ///
    /// 시계가 뒤로 간 경우(SNTP 보정 등) 음수 대신 0을 돌려줍니다.
    pub fn age_s(&self, now: u64) -> Option<u64> {
        self.last_seen.map(|t| now.saturating_sub(t))
    }

    /// 배터리를 뺀 네 측정값이 모두 채워졌는지.
    ///
    /// 배터리는 HHCC 펌웨어에 따라 광고에 실리지 않으므로 조건에서 뺍니다.
    pub fn is_complete(&self) -> bool {
        self.moisture_pct.is_some()
            && self.temperature_deci_c.is_some()
            && self.illuminance_lux.is_some()
            && self.conductivity_us_cm.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mibeacon::Measurement;

    #[test]
    fn 초기_상태는_전부_비어있다() {
        let s = SensorState::new();
        assert_eq!(s.moisture_pct, None);
        assert_eq!(s.last_seen, None);
        assert_eq!(s.age_s(1000), None);
        assert!(!s.is_complete());
    }

    #[test]
    fn 측정값을_적용하면_해당_필드만_채워진다() {
        let mut s = SensorState::new();
        s.apply(Measurement::MoisturePct(32), 1000);
        assert_eq!(s.moisture_pct, Some(32));
        assert_eq!(s.temperature_deci_c, None);
        assert_eq!(s.last_seen, Some(1000));
    }

    #[test]
    fn 나중_값이_이전_값을_덮어쓴다() {
        let mut s = SensorState::new();
        s.apply(Measurement::MoisturePct(32), 1000);
        s.apply(Measurement::MoisturePct(45), 1060);
        assert_eq!(s.moisture_pct, Some(45));
        assert_eq!(s.last_seen, Some(1060));
    }

    #[test]
    fn 네_값이_다_차야_완성이다() {
        let mut s = SensorState::new();
        s.apply_all(
            &[
                Measurement::MoisturePct(32),
                Measurement::TemperatureDeciC(224),
                Measurement::IlluminanceLux(1820),
            ],
            1000,
        );
        assert!(!s.is_complete());
        s.apply(Measurement::ConductivityUsCm(340), 1010);
        assert!(s.is_complete());
    }

    #[test]
    fn 배터리는_완성_조건에_넣지_않는다() {
        // HHCC 펌웨어에 따라 배터리가 광고에 안 실릴 수 있습니다.
        let mut s = SensorState::new();
        s.apply_all(
            &[
                Measurement::MoisturePct(32),
                Measurement::TemperatureDeciC(224),
                Measurement::IlluminanceLux(1820),
                Measurement::ConductivityUsCm(340),
            ],
            1000,
        );
        assert!(s.is_complete());
        assert_eq!(s.battery_pct, None);
    }

    #[test]
    fn 경과시간을_계산한다() {
        let mut s = SensorState::new();
        s.apply(Measurement::MoisturePct(32), 1000);
        assert_eq!(s.age_s(1047), Some(47));
    }

    #[test]
    fn 시계가_뒤로_가도_경과시간은_0이다() {
        let mut s = SensorState::new();
        s.apply(Measurement::MoisturePct(32), 1000);
        assert_eq!(s.age_s(900), Some(0));
    }
}
