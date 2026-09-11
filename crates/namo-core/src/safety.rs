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

/// 누수 센서 하나의 최근 보고.
///
/// 배터리로 도는 Zigbee 센서는 이벤트가 있을 때만 값을 보냅니다. 그래서
/// "값이 오래됐다"와 "센서가 죽었다"를 구분하려면 생존 여부가 따로 필요합니다.
#[derive(Debug, Default, PartialEq, Eq, Clone, Copy)]
pub struct LeakSensor {
    /// 마지막으로 보고된 누수 여부. `None`이면 아직 한 번도 못 받았습니다.
    pub detected: Option<bool>,
    /// 마지막 보고를 받은 시각.
    pub updated_at: Option<u64>,
    /// 게이트웨이가 판정한 센서 생존 여부. `None`이면 아직 모릅니다.
    pub available: Option<bool>,
}

impl LeakSensor {
    /// 이 센서 하나의 판정.
    ///
    /// 죽었다고 알려진 센서는 값이 남아 있어도 믿지 않습니다. 마지막으로
    /// 받은 "누수 없음"은 살아 있던 시점의 이야기이기 때문입니다.
    pub fn state(&self) -> Leak {
        if self.available == Some(false) {
            return Leak::Unknown;
        }
        match self.detected {
            Some(true) => Leak::Detected,
            Some(false) => Leak::None,
            None => Leak::Unknown,
        }
    }
}

/// 여러 센서를 하나의 판정으로 합칩니다.
///
/// 하나라도 누수를 보면 `Detected`입니다. 누수가 없더라도 **하나라도 상태를
/// 모르면** `Unknown`입니다. "한쪽은 멀쩡하니 괜찮겠지"가 아니라 "모르는 곳이
/// 있으면 모른다"로 갑니다.
pub fn combine_leak(sensors: &[LeakSensor]) -> Leak {
    let mut saw_unknown = false;
    for sensor in sensors {
        match sensor.state() {
            Leak::Detected => return Leak::Detected,
            Leak::Unknown => saw_unknown = true,
            Leak::None => {}
        }
    }
    if sensors.is_empty() || saw_unknown {
        Leak::Unknown
    } else {
        Leak::None
    }
}

/// 신선도를 판단할 기준 시각. 센서 중 **가장 오래된** 보고를 씁니다.
///
/// 한쪽만 최근에 보고했다고 전체가 신선하다고 볼 수 없습니다. 하나라도
/// 받은 적이 없으면 `None`이고, 그 경우 안전 판정이 거부합니다.
pub fn oldest_update(sensors: &[LeakSensor]) -> Option<u64> {
    let mut oldest = None;
    for sensor in sensors {
        let t = sensor.updated_at?;
        oldest = Some(match oldest {
            None => t,
            Some(prev) if t < prev => t,
            Some(prev) => prev,
        });
    }
    oldest
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

impl RejectReason {
    /// MQTT 결과 페이로드의 `reason` 값. 스펙 `docs/pilot-design.md` §5.4.
    ///
    /// 게이트웨이가 문자열로 분기하므로 값이 바뀌면 계약이 깨집니다.
    pub const fn as_str(&self) -> &'static str {
        match self {
            RejectReason::ClockUnsynced => "clock_unsynced",
            RejectReason::Locked => "locked",
            RejectReason::AlreadyRunning => "already_running",
            RejectReason::DoseTooLarge => "dose_too_large",
            RejectReason::LeakDetected => "leak_detected",
            RejectReason::LeakStale => "leak_stale",
            RejectReason::ReservoirEmpty => "reservoir_empty",
            RejectReason::ReservoirUnknown => "reservoir_unknown",
            RejectReason::Cooldown => "cooldown",
            RejectReason::DailyLimit => "daily_limit",
        }
    }
}

impl AbortReason {
    /// MQTT 결과 페이로드의 `reason` 값. 거부 사유와 같은 이름 공간을 씁니다.
    pub const fn as_str(&self) -> &'static str {
        match self {
            AbortReason::LeakDetected => "leak_detected",
            AbortReason::ReservoirEmpty => "reservoir_empty",
        }
    }
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
        // Aqara 누수센서는 이벤트가 있을 때만 값을 보내고, 주기 보고는 약
        // 1시간 간격입니다. 처음 잡았던 300초는 센서가 자주 보고한다는
        // 가정에서 나온 값이라, 실제로는 한 시간 중 5분만 급수가 가능한
        // 상태가 됐습니다. 보고 주기에 여유를 더해 90분으로 잡습니다.
        //
        // 센서 생존은 Zigbee2MQTT의 availability로 따로 판정합니다. 이
        // 값은 게이트웨이가 통째로 죽었을 때를 대비한 2차 방어입니다.
        leak_max_age_s: 5_400,
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

    fn sensor(detected: Option<bool>, at: Option<u64>, available: Option<bool>) -> LeakSensor {
        LeakSensor {
            detected,
            updated_at: at,
            available,
        }
    }

    #[test]
    fn 둘_다_정상이면_누수없음() {
        let s = [
            sensor(Some(false), Some(100), Some(true)),
            sensor(Some(false), Some(120), Some(true)),
        ];
        assert_eq!(combine_leak(&s), Leak::None);
    }

    #[test]
    fn 하나라도_누수면_감지() {
        let s = [
            sensor(Some(false), Some(100), Some(true)),
            sensor(Some(true), Some(120), Some(true)),
        ];
        assert_eq!(combine_leak(&s), Leak::Detected);
    }

    #[test]
    fn 하나라도_모르면_unknown() {
        let s = [
            sensor(Some(false), Some(100), Some(true)),
            sensor(None, Some(120), Some(true)),
        ];
        assert_eq!(combine_leak(&s), Leak::Unknown);
    }

    /// 센서가 죽었다고 알려지면 마지막 값이 무엇이든 믿지 않습니다.
    #[test]
    fn 죽은_센서의_값은_믿지_않는다() {
        let s = [sensor(Some(false), Some(100), Some(false))];
        assert_eq!(combine_leak(&s), Leak::Unknown);
    }

    /// 죽은 센서가 누수를 보고한 상태였다면 그것도 unknown입니다. 다만
    /// unknown도 급수를 막으므로 안전 방향은 유지됩니다.
    #[test]
    fn 죽은_센서가_누수중이어도_unknown() {
        let s = [sensor(Some(true), Some(100), Some(false))];
        assert_eq!(combine_leak(&s), Leak::Unknown);
    }

    /// 살아 있는 센서의 누수가 죽은 센서보다 우선합니다.
    #[test]
    fn 살아있는_센서의_누수가_우선한다() {
        let s = [
            sensor(Some(true), Some(100), Some(true)),
            sensor(Some(false), Some(120), Some(false)),
        ];
        assert_eq!(combine_leak(&s), Leak::Detected);
    }

    #[test]
    fn 센서가_없으면_unknown() {
        assert_eq!(combine_leak(&[]), Leak::Unknown);
    }

    /// 생존 여부를 아직 모르는 것(None)은 죽은 것과 다릅니다. 값이 있으면
    /// 그 값을 씁니다.
    #[test]
    fn 생존여부_미상은_값을_그대로_쓴다() {
        let s = [sensor(Some(false), Some(100), None)];
        assert_eq!(combine_leak(&s), Leak::None);
    }

    #[test]
    fn 가장_오래된_보고를_기준으로_삼는다() {
        let s = [
            sensor(Some(false), Some(500), Some(true)),
            sensor(Some(false), Some(100), Some(true)),
        ];
        assert_eq!(oldest_update(&s), Some(100));
    }

    #[test]
    fn 하나라도_보고가_없으면_기준시각이_없다() {
        let s = [
            sensor(Some(false), Some(500), Some(true)),
            sensor(None, None, Some(true)),
        ];
        assert_eq!(oldest_update(&s), None);
    }

    /// 설계 문서 §5.4가 나열한 값과 정확히 일치해야 합니다. 게이트웨이가
    /// 이 문자열로 분기하므로 오타가 나면 조용히 어긋납니다.
    #[test]
    fn 거부사유_문자열이_계약과_일치한다() {
        let pairs = [
            (RejectReason::ClockUnsynced, "clock_unsynced"),
            (RejectReason::Locked, "locked"),
            (RejectReason::AlreadyRunning, "already_running"),
            (RejectReason::DoseTooLarge, "dose_too_large"),
            (RejectReason::LeakDetected, "leak_detected"),
            (RejectReason::LeakStale, "leak_stale"),
            (RejectReason::ReservoirEmpty, "reservoir_empty"),
            (RejectReason::ReservoirUnknown, "reservoir_unknown"),
            (RejectReason::Cooldown, "cooldown"),
            (RejectReason::DailyLimit, "daily_limit"),
        ];
        for (reason, expected) in pairs {
            assert_eq!(reason.as_str(), expected);
        }
    }

    #[test]
    fn 중단사유는_거부사유와_같은_이름을_쓴다() {
        assert_eq!(
            AbortReason::LeakDetected.as_str(),
            RejectReason::LeakDetected.as_str()
        );
        assert_eq!(
            AbortReason::ReservoirEmpty.as_str(),
            RejectReason::ReservoirEmpty.as_str()
        );
    }

    /// ttl_expired는 계약에 없습니다. 만료된 명령은 결과를 발행하지 않고
    /// 조용히 버리기 때문입니다(§4.6).
    #[test]
    fn ttl_expired는_사유에_없다() {
        let all = [
            RejectReason::ClockUnsynced,
            RejectReason::Locked,
            RejectReason::AlreadyRunning,
            RejectReason::DoseTooLarge,
            RejectReason::LeakDetected,
            RejectReason::LeakStale,
            RejectReason::ReservoirEmpty,
            RejectReason::ReservoirUnknown,
            RejectReason::Cooldown,
            RejectReason::DailyLimit,
        ];
        assert!(all.iter().all(|r| r.as_str() != "ttl_expired"));
    }

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
        // 게이트웨이가 죽어 허용 시간을 넘긴 상황.
        //
        // 임계값을 하드코딩하지 않고 Limits에서 가져옵니다. 예전에는 300초를
        // 그대로 적어뒀는데, 센서 특성에 맞춰 정책을 바꾸자 테스트가 옛
        // 숫자를 지키느라 실패했습니다. 검증하려는 것은 특정 초가 아니라
        // "임계값을 넘으면 거부한다"입니다.
        let mut i = 정상();
        i.leak_updated_at = Some(10_000 - (Limits::DEFAULT.leak_max_age_s + 1));
        assert_eq!(evaluate(&i, &Limits::DEFAULT), Err(RejectReason::LeakStale));
    }

    #[test]
    fn 누수정보가_임계값_이내면_통과한다() {
        // 경계 바로 안쪽. 여기서 거부되면 정상 운전이 막힙니다.
        let mut i = 정상();
        i.leak_updated_at = Some(10_000 - Limits::DEFAULT.leak_max_age_s);
        assert_eq!(evaluate(&i, &Limits::DEFAULT), Ok(()));
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
