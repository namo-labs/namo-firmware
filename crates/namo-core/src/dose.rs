//! 급수량(mL)과 펌프 구동시간(ms) 사이의 환산.

/// 펌프 유량 보정값.
///
/// 카탈로그 값이 아니라 실제 설치 높이에서 실측한 값을 씁니다.
/// (`docs/bring-up-guide.md` Stage 4)
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct Calibration {
    /// 초당 토출량, 마이크로리터.
    pub ul_per_second: u32,
}

impl Calibration {
    /// 사람이 쓰는 mL/s 값을 100배 정수로 받아 만듭니다. 6.50 mL/s → 650.
    pub const fn from_ml_per_second_x100(hundredths: u32) -> Self {
        Self {
            ul_per_second: hundredths * 10,
        }
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum DoseError {
    /// 보정값이 0이라 환산이 불가능합니다. Stage 4 실측을 하지 않은 상태입니다.
    InvalidCalibration,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct PumpPlan {
    pub pump_ms: u32,
    /// 하드리밋에 걸려 요청보다 짧게 잘렸는지.
    pub clamped: bool,
}

/// 급수량을 펌프 구동시간으로 환산하고 하드리밋으로 자릅니다.
///
/// 자르는 것은 안전장치이므로 오류가 아닙니다. 대신 `clamped`로 알립니다.
pub fn plan(dose_ml: u32, cal: Calibration, max_pump_ms: u32) -> Result<PumpPlan, DoseError> {
    if cal.ul_per_second == 0 {
        return Err(DoseError::InvalidCalibration);
    }

    // ms = dose_ml * 1_000_000 / ul_per_second
    // u64로 계산해 오버플로를 피하고, 반올림을 위해 분모의 절반을 더합니다.
    let numerator = dose_ml as u64 * 1_000_000;
    let denominator = cal.ul_per_second as u64;
    let raw_ms = (numerator + denominator / 2) / denominator;

    if raw_ms > max_pump_ms as u64 {
        Ok(PumpPlan {
            pump_ms: max_pump_ms,
            clamped: true,
        })
    } else {
        Ok(PumpPlan {
            pump_ms: raw_ms as u32,
            clamped: false,
        })
    }
}

/// 구동시간으로부터 실제 토출량을 추정합니다.
///
/// 유량계 실측이 아니라 보정값 기반 추정치입니다.
pub fn estimated_ml(pump_ms: u32, cal: Calibration) -> u32 {
    let ul = pump_ms as u64 * cal.ul_per_second as u64 / 1_000;
    (ul / 1_000) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 6.5 mL/s — Stage 4 실측을 가정한 값.
    const CAL: Calibration = Calibration { ul_per_second: 6500 };
    const MAX_MS: u32 = 20_000;

    #[test]
    fn 백밀리리터를_구동시간으로_바꾼다() {
        // 100mL / 6.5mL/s = 15.3846...초 → 반올림 15385ms
        let p = plan(100, CAL, MAX_MS).unwrap();
        assert_eq!(p.pump_ms, 15_385);
        assert!(!p.clamped);
    }

    #[test]
    fn 하드리밋을_넘으면_잘라내고_표시한다() {
        // 300mL는 46초가 필요하지만 20초에서 잘립니다.
        let p = plan(300, CAL, MAX_MS).unwrap();
        assert_eq!(p.pump_ms, MAX_MS);
        assert!(p.clamped);
    }

    #[test]
    fn 영_밀리리터는_구동하지_않는다() {
        let p = plan(0, CAL, MAX_MS).unwrap();
        assert_eq!(p.pump_ms, 0);
        assert!(!p.clamped);
    }

    #[test]
    fn 보정값이_영이면_거부한다() {
        let cal = Calibration { ul_per_second: 0 };
        assert_eq!(plan(100, cal, MAX_MS), Err(DoseError::InvalidCalibration));
    }

    #[test]
    fn 구동시간에서_추정량을_역산한다() {
        assert_eq!(estimated_ml(15_385, CAL), 100);
    }

    #[test]
    fn 잘린_구동의_추정량은_요청보다_적다() {
        let p = plan(300, CAL, MAX_MS).unwrap();
        let actual = estimated_ml(p.pump_ms, CAL);
        assert_eq!(actual, 130);
        assert!(actual < 300);
    }

    #[test]
    fn 소수점_이하는_사사오입한다() {
        // 10mL / 6.5 = 1.538...초 → 1538ms
        assert_eq!(plan(10, CAL, MAX_MS).unwrap().pump_ms, 1_538);
    }

    #[test]
    fn 소수_보정값을_백분의일_단위로_받는다() {
        // 6.50 mL/s
        assert_eq!(Calibration::from_ml_per_second_x100(650).ul_per_second, 6_500);
    }

    #[test]
    fn 큰_요청에도_오버플로가_없다() {
        let p = plan(u32::MAX, CAL, MAX_MS).unwrap();
        assert_eq!(p.pump_ms, MAX_MS);
        assert!(p.clamped);
    }
}
