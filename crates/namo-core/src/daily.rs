//! 일일 급수량 누적과 자정 리셋. 스펙 `docs/pilot-design.md` §4.5의 S7.
//!
//! 리셋 기준은 **로컬 시각의 자정**입니다. UTC 자정으로 재면 한국에서는
//! 오전 9시에 한도가 풀려, 밤에 한도를 다 쓴 화분이 아침에 또 물을 받습니다.

/// 한국 표준시. UTC보다 9시간 빠릅니다.
pub const KST_OFFSET_S: i64 = 9 * 3600;

/// epoch 초를 로컬 기준 "며칠째"로 바꿉니다.
///
/// 값 자체에는 의미가 없고, 두 시각이 같은 날인지 비교하는 데만 씁니다.
pub fn local_day_index(now_epoch_s: u64, utc_offset_s: i64) -> i64 {
    // i64로 올려서 계산합니다. 음수 오프셋(서반구)에서도 바닥 나눗셈이
    // 맞아떨어져야 합니다.
    let local = now_epoch_s as i64 + utc_offset_s;
    local.div_euclid(86_400)
}

/// 오늘 누적 급수량. 날짜가 바뀌면 자동으로 0부터 다시 셉니다.
#[derive(Debug, Default, Clone, Copy)]
pub struct DailyTotal {
    /// 지금 누적치가 속한 날. `None`이면 아직 한 번도 더하지 않았습니다.
    day: Option<i64>,
    ml: u32,
}

impl DailyTotal {
    pub const fn new() -> Self {
        Self { day: None, ml: 0 }
    }

    /// 오늘 누적치를 돌려줍니다. 날짜가 넘어갔으면 0입니다.
    ///
    /// 상태를 바꾸지 않으므로, 안전 판정처럼 읽기만 하는 곳에서 써도
    /// 부작용이 없습니다.
    pub fn today_ml(&self, now_epoch_s: u64, utc_offset_s: i64) -> u32 {
        match self.day {
            Some(day) if day == local_day_index(now_epoch_s, utc_offset_s) => self.ml,
            _ => 0,
        }
    }

    /// 급수량을 더합니다. 날짜가 넘어갔으면 먼저 0으로 되돌린 뒤 더합니다.
    pub fn add(&mut self, ml: u32, now_epoch_s: u64, utc_offset_s: i64) {
        let today = local_day_index(now_epoch_s, utc_offset_s);
        if self.day != Some(today) {
            self.day = Some(today);
            self.ml = 0;
        }
        self.ml = self.ml.saturating_add(ml);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-09-07 00:00:00 KST = 2026-09-06 15:00:00 UTC
    const KST_MIDNIGHT: u64 = 1_788_706_800;

    #[test]
    fn 같은_날_누적된다() {
        let mut d = DailyTotal::new();
        d.add(100, KST_MIDNIGHT, KST_OFFSET_S);
        d.add(50, KST_MIDNIGHT + 3600, KST_OFFSET_S);
        assert_eq!(d.today_ml(KST_MIDNIGHT + 3600, KST_OFFSET_S), 150);
    }

    #[test]
    fn 자정을_넘기면_리셋된다() {
        let mut d = DailyTotal::new();
        d.add(400, KST_MIDNIGHT + 3600, KST_OFFSET_S);
        // 다음 날 같은 시각
        let tomorrow = KST_MIDNIGHT + 86_400 + 3600;
        assert_eq!(d.today_ml(tomorrow, KST_OFFSET_S), 0);
    }

    #[test]
    fn 자정_직전과_직후가_다른_날이다() {
        let before = KST_MIDNIGHT - 1;
        let after = KST_MIDNIGHT;
        assert_ne!(
            local_day_index(before, KST_OFFSET_S),
            local_day_index(after, KST_OFFSET_S)
        );
    }

    #[test]
    fn utc자정은_경계가_아니다() {
        // UTC 자정(= KST 오전 9시)에는 날짜가 바뀌면 안 됩니다.
        let utc_midnight = 1_788_739_200; // 2026-09-07 00:00:00 UTC
        assert_eq!(
            local_day_index(utc_midnight - 1, KST_OFFSET_S),
            local_day_index(utc_midnight, KST_OFFSET_S)
        );
    }

    #[test]
    fn 날짜가_바뀐_뒤_더하면_새로_센다() {
        let mut d = DailyTotal::new();
        d.add(400, KST_MIDNIGHT, KST_OFFSET_S);
        let tomorrow = KST_MIDNIGHT + 86_400;
        d.add(100, tomorrow, KST_OFFSET_S);
        assert_eq!(d.today_ml(tomorrow, KST_OFFSET_S), 100);
    }

    #[test]
    fn 처음에는_0이다() {
        let d = DailyTotal::new();
        assert_eq!(d.today_ml(KST_MIDNIGHT, KST_OFFSET_S), 0);
    }

    #[test]
    fn 읽기는_상태를_바꾸지_않는다() {
        let mut d = DailyTotal::new();
        d.add(100, KST_MIDNIGHT, KST_OFFSET_S);
        let tomorrow = KST_MIDNIGHT + 86_400;
        assert_eq!(d.today_ml(tomorrow, KST_OFFSET_S), 0);
        // 내일 기준으로 읽었다고 해서 어제 누적치가 지워지면 안 됩니다.
        assert_eq!(d.today_ml(KST_MIDNIGHT, KST_OFFSET_S), 100);
    }

    #[test]
    fn 음수_오프셋에서도_경계가_맞다() {
        // UTC-5. 로컬 자정은 UTC 05:00입니다.
        let offset = -5 * 3600;
        let local_midnight = 1_788_739_200 + 5 * 3600;
        assert_ne!(
            local_day_index(local_midnight - 1, offset),
            local_day_index(local_midnight, offset)
        );
    }

    #[test]
    fn 누적이_넘치지_않는다() {
        let mut d = DailyTotal::new();
        d.add(u32::MAX, KST_MIDNIGHT, KST_OFFSET_S);
        d.add(1000, KST_MIDNIGHT, KST_OFFSET_S);
        assert_eq!(d.today_ml(KST_MIDNIGHT, KST_OFFSET_S), u32::MAX);
    }
}
