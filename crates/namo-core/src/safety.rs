//! 급수 허용 여부 판정. 스펙 `docs/pilot-design.md` §4.5.

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Reservoir {
    Ok,
    Empty,
    /// 아직 안정된 값을 읽지 못했습니다(부팅 직후 디바운스 등).
    Unknown,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Leak {
    None,
    Detected,
    Unknown,
}

/// 급수를 거부한 사유. 판정 순서가 곧 우선순위입니다.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum RejectReason {
    ClockUnsynced,
    Locked,
    AlreadyRunning,
    DoseTooLarge,
    LeakDetected,
    LeakStale,
    ReservoirEmpty,
    ReservoirUnknown,
    Cooldown,
    DailyLimit,
}

/// 구동 중 급수를 중단한 사유.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum AbortReason {
    LeakDetected,
    ReservoirEmpty,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct Limits {
    pub max_dose_ml: u32,
    pub daily_limit_ml: u32,
    pub cooldown_s: u64,
    pub leak_max_age_s: u64,
}

impl Limits {
    /// 스펙 §4.5의 기본값. 바질 15cm 화분 기준이며 운영하며 조정합니다.
    pub const DEFAULT: Limits = Limits {
        max_dose_ml: 300,
        daily_limit_ml: 500,
        cooldown_s: 1_800,
        leak_max_age_s: 300,
    };
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct SafetyInput {
    /// 현재 시각. SNTP 동기 전이면 `None`입니다.
    pub now: Option<u64>,
    pub reservoir: Reservoir,
    pub leak: Leak,
    /// 누수 상태를 마지막으로 갱신받은 시각.
    pub leak_updated_at: Option<u64>,
    /// 누수 감지로 잠긴 상태인지.
    pub locked: bool,
    /// 이미 급수가 진행 중인지.
    pub running: bool,
    pub last_completed_at: Option<u64>,
    pub today_ml: u32,
    pub dose_ml: u32,
}

/// 급수를 시작해도 되는지 판정합니다.
///
/// 상태를 모르면 거부합니다(fail-closed). 물을 못 주는 것보다
/// 누수를 모른 채 주는 것이 훨씬 나쁩니다.
pub fn evaluate(input: &SafetyInput, limits: &Limits) -> Result<(), RejectReason> {
    // 시각을 모르면 TTL도 쿨다운도 판정할 수 없습니다.
    let now = match input.now {
        Some(t) => t,
        None => return Err(RejectReason::ClockUnsynced),
    };

    if input.locked {
        return Err(RejectReason::Locked);
    }
    if input.running {
        return Err(RejectReason::AlreadyRunning);
    }
    if input.dose_ml > limits.max_dose_ml {
        return Err(RejectReason::DoseTooLarge);
    }

    match input.leak {
        Leak::Detected => return Err(RejectReason::LeakDetected),
        Leak::Unknown => return Err(RejectReason::LeakStale),
        Leak::None => {}
    }
    match input.leak_updated_at {
        None => return Err(RejectReason::LeakStale),
        Some(t) if now.saturating_sub(t) > limits.leak_max_age_s => {
            return Err(RejectReason::LeakStale)
        }
        Some(_) => {}
    }

    match input.reservoir {
        Reservoir::Empty => return Err(RejectReason::ReservoirEmpty),
        Reservoir::Unknown => return Err(RejectReason::ReservoirUnknown),
        Reservoir::Ok => {}
    }

    if let Some(last) = input.last_completed_at {
        if now.saturating_sub(last) < limits.cooldown_s {
            return Err(RejectReason::Cooldown);
        }
    }

    if input.today_ml.saturating_add(input.dose_ml) > limits.daily_limit_ml {
        return Err(RejectReason::DailyLimit);
    }

    Ok(())
}

/// 쿨다운으로 막혀 있다면 해제 예상 시각을 돌려줍니다.
pub fn next_available_at(input: &SafetyInput, limits: &Limits) -> Option<u64> {
    let now = input.now?;
    let last = input.last_completed_at?;
    let unlock_at = last + limits.cooldown_s;
    if now < unlock_at {
        Some(unlock_at)
    } else {
        None
    }
}

/// 구동 중 즉시 중단해야 하는지 판정합니다.
///
/// 시작 조건(`evaluate`)보다 느슨합니다. 이미 물이 나가는 중에
/// 정보 두절만으로 멈추면 튜브에 물이 남아 상태 추정이 더 어려워집니다.
/// 확실한 위험 신호에만 멈춥니다.
pub fn abort_reason(reservoir: Reservoir, leak: Leak) -> Option<AbortReason> {
    if leak == Leak::Detected {
        return Some(AbortReason::LeakDetected);
    }
    if reservoir == Reservoir::Empty {
        return Some(AbortReason::ReservoirEmpty);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 모든 조건이 정상인 기준 입력.
    fn 정상() -> SafetyInput {
        SafetyInput {
            now: Some(10_000),
            reservoir: Reservoir::Ok,
            leak: Leak::None,
            leak_updated_at: Some(9_990),
            locked: false,
            running: false,
            last_completed_at: None,
            today_ml: 0,
            dose_ml: 100,
        }
    }

    #[test]
    fn 정상_상태는_허용된다() {
        assert_eq!(evaluate(&정상(), &Limits::DEFAULT), Ok(()));
    }

    #[test]
    fn 시각_미동기면_거부한다() {
        let mut i = 정상();
        i.now = None;
        assert_eq!(evaluate(&i, &Limits::DEFAULT), Err(RejectReason::ClockUnsynced));
    }

    #[test]
    fn 잠금상태면_거부한다() {
        let mut i = 정상();
        i.locked = true;
        assert_eq!(evaluate(&i, &Limits::DEFAULT), Err(RejectReason::Locked));
    }

    #[test]
    fn 진행중이면_거부한다() {
        let mut i = 정상();
        i.running = true;
        assert_eq!(evaluate(&i, &Limits::DEFAULT), Err(RejectReason::AlreadyRunning));
    }

    #[test]
    fn 일회_상한을_넘으면_거부한다() {
        let mut i = 정상();
        i.dose_ml = 400;
        assert_eq!(evaluate(&i, &Limits::DEFAULT), Err(RejectReason::DoseTooLarge));
    }

    #[test]
    fn 일회_상한_경계값은_허용한다() {
        let mut i = 정상();
        i.dose_ml = Limits::DEFAULT.max_dose_ml;
        assert_eq!(evaluate(&i, &Limits::DEFAULT), Ok(()));
    }

    #[test]
    fn 누수가_감지되면_거부한다() {
        let mut i = 정상();
        i.leak = Leak::Detected;
        assert_eq!(evaluate(&i, &Limits::DEFAULT), Err(RejectReason::LeakDetected));
    }

    #[test]
    fn 누수상태를_모르면_거부한다() {
        let mut i = 정상();
        i.leak = Leak::Unknown;
        assert_eq!(evaluate(&i, &Limits::DEFAULT), Err(RejectReason::LeakStale));
    }

    #[test]
    fn 누수정보가_오래되면_거부한다() {
        // 게이트웨이가 죽어 5분 넘게 갱신이 없는 상황.
        let mut i = 정상();
        i.leak_updated_at = Some(10_000 - 301);
        assert_eq!(evaluate(&i, &Limits::DEFAULT), Err(RejectReason::LeakStale));
    }

    #[test]
    fn 누수정보를_한_번도_못_받으면_거부한다() {
        let mut i = 정상();
        i.leak_updated_at = None;
        assert_eq!(evaluate(&i, &Limits::DEFAULT), Err(RejectReason::LeakStale));
    }

    #[test]
    fn 물통이_비면_거부한다() {
        let mut i = 정상();
        i.reservoir = Reservoir::Empty;
        assert_eq!(evaluate(&i, &Limits::DEFAULT), Err(RejectReason::ReservoirEmpty));
    }

    #[test]
    fn 물통상태를_모르면_거부한다() {
        let mut i = 정상();
        i.reservoir = Reservoir::Unknown;
        assert_eq!(evaluate(&i, &Limits::DEFAULT), Err(RejectReason::ReservoirUnknown));
    }

    #[test]
    fn 쿨다운_중이면_거부한다() {
        let mut i = 정상();
        i.last_completed_at = Some(10_000 - 60);
        assert_eq!(evaluate(&i, &Limits::DEFAULT), Err(RejectReason::Cooldown));
    }

    #[test]
    fn 쿨다운이_끝나면_허용한다() {
        let mut i = 정상();
        i.last_completed_at = Some(10_000 - Limits::DEFAULT.cooldown_s);
        assert_eq!(evaluate(&i, &Limits::DEFAULT), Ok(()));
    }

    #[test]
    fn 일일한도를_넘기면_거부한다() {
        let mut i = 정상();
        i.today_ml = 450;
        i.dose_ml = 100; // 합계 550 > 500
        assert_eq!(evaluate(&i, &Limits::DEFAULT), Err(RejectReason::DailyLimit));
    }

    #[test]
    fn 일일한도_경계값은_허용한다() {
        let mut i = 정상();
        i.today_ml = 400;
        i.dose_ml = 100; // 합계 정확히 500
        assert_eq!(evaluate(&i, &Limits::DEFAULT), Ok(()));
    }

    #[test]
    fn 누수가_물통보다_우선한다() {
        // 둘 다 문제일 때 더 위험한 쪽을 사유로 보여줍니다.
        let mut i = 정상();
        i.leak = Leak::Detected;
        i.reservoir = Reservoir::Empty;
        assert_eq!(evaluate(&i, &Limits::DEFAULT), Err(RejectReason::LeakDetected));
    }

    #[test]
    fn 잠금이_누수보다_우선한다() {
        let mut i = 정상();
        i.locked = true;
        i.leak = Leak::Detected;
        assert_eq!(evaluate(&i, &Limits::DEFAULT), Err(RejectReason::Locked));
    }

    #[test]
    fn 다음_급수_가능시각을_계산한다() {
        let mut i = 정상();
        i.last_completed_at = Some(10_000 - 60);
        assert_eq!(next_available_at(&i, &Limits::DEFAULT), Some(10_000 - 60 + 1800));
    }

    #[test]
    fn 쿨다운이_아니면_다음_가능시각은_없다() {
        assert_eq!(next_available_at(&정상(), &Limits::DEFAULT), None);
    }

    #[test]
    fn 구동중_누수는_중단사유다() {
        assert_eq!(
            abort_reason(Reservoir::Ok, Leak::Detected),
            Some(AbortReason::LeakDetected)
        );
    }

    #[test]
    fn 구동중_물통고갈은_중단사유다() {
        assert_eq!(
            abort_reason(Reservoir::Empty, Leak::None),
            Some(AbortReason::ReservoirEmpty)
        );
    }

    #[test]
    fn 구동중_정상이면_중단하지_않는다() {
        assert_eq!(abort_reason(Reservoir::Ok, Leak::None), None);
    }

    #[test]
    fn 구동중_상태불명은_중단하지_않는다() {
        // 이미 물이 나가는 중에 정보가 끊긴 것으로 멈추면
        // 튜브에 물이 남아 오히려 예측이 어려워집니다.
        // 시작을 막는 것(evaluate)과 진행 중 중단은 기준이 다릅니다.
        assert_eq!(abort_reason(Reservoir::Unknown, Leak::Unknown), None);
    }
}
