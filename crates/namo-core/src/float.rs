//! 플로트 스위치 디바운스. 스펙 `docs/pilot-design.md` §3.1.
//!
//! 스위치 접점은 붙고 떨어지는 순간 수 ms 동안 여러 번 튑니다. 그대로 읽으면
//! 물통을 흔들 때마다 상태가 요동칩니다. 같은 값이 연속으로 일정 횟수 나와야
//! 확정하는 방식으로 걸러냅니다.

use crate::safety::Reservoir;

/// 상태를 확정하는 데 필요한 연속 일치 횟수.
///
/// 20ms 주기로 폴링하면 5회는 100ms에 해당합니다. 스펙의 디바운스 시간이
/// 바뀌면 폴링 주기와 함께 조정합니다.
pub const DEFAULT_SAMPLES: u8 = 5;

/// 플로트 스위치 입력을 디바운스합니다.
///
/// 확정된 값이 나오기 전에는 [`Reservoir::Unknown`]을 돌려줍니다. 안전 판정은
/// `Unknown`에서 급수를 거부하므로(fail-closed) 부팅 직후 잘못된 급수가
/// 일어나지 않습니다.
#[derive(Debug, Clone)]
pub struct Debouncer {
    stable: Reservoir,
    /// 지금 연속으로 관찰 중인 원시 입력값. `None`이면 아직 관찰 시작 전입니다.
    candidate: Option<bool>,
    streak: u8,
    required: u8,
}

impl Debouncer {
    /// `required`번 연속 같은 값이 나와야 상태를 확정합니다.
    ///
    /// `required`가 0이면 1로 취급합니다. 디바운스를 끄는 설정 실수가
    /// 0으로 나타나더라도 최소 한 번은 읽고 판단하게 만듭니다.
    pub fn new(required: u8) -> Self {
        Self {
            stable: Reservoir::Unknown,
            candidate: None,
            streak: 0,
            required: required.max(1),
        }
    }

    /// 원시 입력 한 번을 반영하고 현재 확정 상태를 돌려줍니다.
    ///
    /// `is_low`는 GPIO가 LOW로 읽혔는지입니다. 내부 풀업을 쓰고 반대편이
    /// GND이므로, 접점이 닫히면(플로트가 뜨면) LOW입니다. 실측 결과 플로트가
    /// 뜬 상태가 "물 있음"이므로 LOW가 [`Reservoir::Ok`]입니다.
    pub fn update(&mut self, is_low: bool) -> Reservoir {
        if self.candidate == Some(is_low) {
            // 이미 최대치면 더 세지 않습니다. u8 한계를 넘기지 않기 위함입니다.
            self.streak = self.streak.saturating_add(1);
        } else {
            self.candidate = Some(is_low);
            self.streak = 1;
        }

        if self.streak >= self.required {
            self.stable = if is_low {
                Reservoir::Ok
            } else {
                Reservoir::Empty
            };
        }

        self.stable
    }

    /// 마지막으로 확정된 상태. 새 입력을 반영하지 않습니다.
    pub fn state(&self) -> Reservoir {
        self.stable
    }
}

impl Default for Debouncer {
    fn default() -> Self {
        Self::new(DEFAULT_SAMPLES)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 부팅_직후는_unknown이다() {
        let d = Debouncer::default();
        assert_eq!(d.state(), Reservoir::Unknown);
    }

    #[test]
    fn 확정_전까지는_unknown을_유지한다() {
        let mut d = Debouncer::new(5);
        for _ in 0..4 {
            assert_eq!(d.update(true), Reservoir::Unknown);
        }
        assert_eq!(d.update(true), Reservoir::Ok);
    }

    #[test]
    fn 연속_low는_물있음으로_확정된다() {
        let mut d = Debouncer::new(3);
        d.update(true);
        d.update(true);
        assert_eq!(d.update(true), Reservoir::Ok);
    }

    #[test]
    fn 연속_high는_물없음으로_확정된다() {
        let mut d = Debouncer::new(3);
        d.update(false);
        d.update(false);
        assert_eq!(d.update(false), Reservoir::Empty);
    }

    #[test]
    fn 튀는_입력은_확정을_미룬다() {
        let mut d = Debouncer::new(3);
        // 붙었다 떨어졌다를 반복하면 연속 횟수가 계속 초기화됩니다.
        for _ in 0..10 {
            assert_eq!(d.update(true), Reservoir::Unknown);
            assert_eq!(d.update(false), Reservoir::Unknown);
        }
    }

    #[test]
    fn 확정_후_한_번_튀어도_기존_상태를_유지한다() {
        let mut d = Debouncer::new(3);
        for _ in 0..3 {
            d.update(true);
        }
        assert_eq!(d.state(), Reservoir::Ok);

        // 접점이 한 번 튀었다고 바로 empty로 넘어가면, 물통을 건드릴 때마다
        // 급수가 거부됩니다.
        assert_eq!(d.update(false), Reservoir::Ok);
        assert_eq!(d.update(false), Reservoir::Ok);
        assert_eq!(d.update(false), Reservoir::Empty);
    }

    #[test]
    fn 상태가_오갈_수_있다() {
        let mut d = Debouncer::new(2);
        d.update(true);
        assert_eq!(d.update(true), Reservoir::Ok);
        d.update(false);
        assert_eq!(d.update(false), Reservoir::Empty);
        d.update(true);
        assert_eq!(d.update(true), Reservoir::Ok);
    }

    #[test]
    fn 오래_유지해도_카운터가_넘치지_않는다() {
        let mut d = Debouncer::new(3);
        for _ in 0..1000 {
            d.update(true);
        }
        assert_eq!(d.state(), Reservoir::Ok);
    }

    #[test]
    fn required가_0이면_1로_취급한다() {
        let mut d = Debouncer::new(0);
        assert_eq!(d.update(true), Reservoir::Ok);
    }
}
