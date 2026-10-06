//! 네트워크 복구 판정. 스펙 `docs/pilot-design.md` §4.7.
//!
//! 시각은 전부 **부팅 후 경과 초**입니다. SNTP가 안 맞아도 동작해야 하고,
//! 벽시계는 동기화 순간에 뒤로 튈 수 있기 때문입니다.

/// WiFi 재접속 간격(초). 마지막 값에 도달하면 그 간격을 유지합니다.
///
/// 짧은 간격으로 계속 두드리면 공유기가 방어적으로 차단해 오히려 더 늦게
/// 붙습니다.
const BACKOFF_S: [u64; 5] = [2, 5, 10, 20, 30];

/// `attempt`번째(1부터) 시도가 실패한 뒤 기다릴 시간.
pub fn backoff_s(attempt: u32) -> u64 {
    let i = (attempt.saturating_sub(1) as usize).min(BACKOFF_S.len() - 1);
    BACKOFF_S[i]
}

/// 접속이 끊긴 동안 다음 시도 시각을 관리합니다.
#[derive(Debug, Default, Clone, Copy)]
pub struct Reconnect {
    attempt: u32,
    next_at_s: u64,
}

impl Reconnect {
    pub const fn new() -> Self {
        Self {
            attempt: 0,
            next_at_s: 0,
        }
    }

    /// 점검할 때마다 부릅니다. `true`면 지금 접속을 시도해야 합니다.
    pub fn should_try(&mut self, now_s: u64, link_up: bool) -> bool {
        if link_up {
            // 다음에 끊기면 기다림 없이 바로 시도하고 간격도 처음부터 셉니다.
            *self = Self::new();
            return false;
        }
        if now_s < self.next_at_s {
            return false;
        }
        self.attempt = self.attempt.saturating_add(1);
        self.next_at_s = now_s.saturating_add(backoff_s(self.attempt));
        true
    }
}

/// 워치독의 판정.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Verdict {
    /// 그대로 둡니다.
    Keep,
    /// 재부팅합니다.
    Restart,
}

/// MQTT가 오래 안 붙으면 재부팅을 권하는 마지막 방어선.
#[derive(Debug, Clone, Copy)]
pub struct LinkWatchdog {
    restart_after_s: u64,
    down_since_s: Option<u64>,
}

impl LinkWatchdog {
    pub const fn new(restart_after_s: u64) -> Self {
        Self {
            restart_after_s,
            down_since_s: None,
        }
    }

    /// 점검할 때마다 부릅니다.
    ///
    /// 펌프가 도는 중에는 재부팅을 미룹니다. 재부팅해도 펌프는 꺼지지만(S1),
    /// 진행 중인 급수가 중간에 잘리고 결과를 못 남깁니다.
    pub fn observe(&mut self, now_s: u64, mqtt_up: bool, pump_idle: bool) -> Verdict {
        if mqtt_up {
            self.down_since_s = None;
            return Verdict::Keep;
        }

        let since = *self.down_since_s.get_or_insert(now_s);
        let down_for = now_s.saturating_sub(since);
        if down_for >= self.restart_after_s && pump_idle {
            Verdict::Restart
        } else {
            Verdict::Keep
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 백오프는_표를_따라_늘다가_마지막_값을_유지한다() {
        let got: [u64; 8] = core::array::from_fn(|i| backoff_s(i as u32 + 1));
        assert_eq!(got, [2, 5, 10, 20, 30, 30, 30, 30]);
    }

    #[test]
    fn 붙어_있으면_시도하지_않는다() {
        let mut r = Reconnect::new();
        for t in 0..100 {
            assert!(!r.should_try(t, true));
        }
    }

    #[test]
    fn 끊기면_즉시_한_번_시도한다() {
        let mut r = Reconnect::new();
        assert!(!r.should_try(50, true));
        assert!(r.should_try(51, false));
    }

    #[test]
    fn 끊긴_채로_간격을_늘려가며_재시도한다() {
        let mut r = Reconnect::new();
        // 시도 시각: 0, +2, +5, +10, +20, +30, +30
        let mut tries = [0u64; 7];
        let mut n = 0;
        for t in 0..=200 {
            if r.should_try(t, false) && n < tries.len() {
                tries[n] = t;
                n += 1;
            }
        }
        assert_eq!(tries, [0, 2, 7, 17, 37, 67, 97]);
    }

    #[test]
    fn 간격이_지나기_전에는_다시_시도하지_않는다() {
        let mut r = Reconnect::new();
        assert!(r.should_try(0, false));
        assert!(!r.should_try(1, false));
        assert!(r.should_try(2, false));
        assert!(!r.should_try(6, false));
        assert!(r.should_try(7, false));
    }

    #[test]
    fn 붙으면_간격이_처음으로_돌아간다() {
        let mut r = Reconnect::new();
        // 오래 끊겨 간격이 30초까지 늘어난 상태
        for t in 0..=200 {
            r.should_try(t, false);
        }
        assert!(!r.should_try(201, true));

        // 다시 끊기면 즉시 시도하고, 다음은 2초 뒤여야 합니다.
        assert!(r.should_try(300, false));
        assert!(!r.should_try(301, false));
        assert!(r.should_try(302, false));
    }

    const LIMIT: u64 = 900;

    #[test]
    fn 붙어_있으면_재부팅하지_않는다() {
        let mut w = LinkWatchdog::new(LIMIT);
        for t in (0..100_000).step_by(60) {
            assert_eq!(w.observe(t, true, true), Verdict::Keep);
        }
    }

    #[test]
    fn 끊긴_시간이_한도_미만이면_둔다() {
        let mut w = LinkWatchdog::new(LIMIT);
        assert_eq!(w.observe(100, false, true), Verdict::Keep);
        assert_eq!(w.observe(100 + LIMIT - 1, false, true), Verdict::Keep);
    }

    #[test]
    fn 끊긴_시간이_한도에_닿으면_재부팅한다() {
        let mut w = LinkWatchdog::new(LIMIT);
        assert_eq!(w.observe(100, false, true), Verdict::Keep);
        assert_eq!(w.observe(100 + LIMIT, false, true), Verdict::Restart);
    }

    #[test]
    fn 중간에_붙으면_끊긴_시간을_처음부터_센다() {
        let mut w = LinkWatchdog::new(LIMIT);
        assert_eq!(w.observe(0, false, true), Verdict::Keep);
        assert_eq!(w.observe(LIMIT - 10, false, true), Verdict::Keep);
        // 한도 직전에 붙었다가 다시 끊깁니다. 15분이 이어진 게 아닙니다.
        assert_eq!(w.observe(LIMIT - 5, true, true), Verdict::Keep);
        assert_eq!(w.observe(LIMIT, false, true), Verdict::Keep);
        assert_eq!(w.observe(LIMIT + LIMIT - 1, false, true), Verdict::Keep);
        assert_eq!(w.observe(LIMIT + LIMIT, false, true), Verdict::Restart);
    }

    #[test]
    fn 펌프가_도는_동안은_재부팅을_미룬다() {
        let mut w = LinkWatchdog::new(LIMIT);
        assert_eq!(w.observe(0, false, true), Verdict::Keep);
        assert_eq!(w.observe(LIMIT + 5, false, false), Verdict::Keep);
        assert_eq!(w.observe(LIMIT + 6, false, false), Verdict::Keep);
    }

    #[test]
    fn 펌프가_멈추면_미뤄둔_재부팅을_바로_한다() {
        let mut w = LinkWatchdog::new(LIMIT);
        assert_eq!(w.observe(0, false, true), Verdict::Keep);
        assert_eq!(w.observe(LIMIT + 5, false, false), Verdict::Keep);
        assert_eq!(w.observe(LIMIT + 6, false, true), Verdict::Restart);
    }

    #[test]
    fn 시각이_거꾸로_가도_재부팅하지_않는다() {
        let mut w = LinkWatchdog::new(LIMIT);
        assert_eq!(w.observe(5_000, false, true), Verdict::Keep);
        assert_eq!(w.observe(10, false, true), Verdict::Keep);
    }
}
