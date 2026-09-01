//! 모듈을 조합한 급수 사이클 시나리오.
//! `docs/bring-up-guide.md` Stage 8의 검증 항목에 대응합니다.

use namo_core::command::{check_ttl, CommandId, RecentIds, TtlVerdict, WaterCommand};
use namo_core::dose::{estimated_ml, plan, Calibration};
use namo_core::mibeacon;
use namo_core::safety::{
    abort_reason, evaluate, AbortReason, Leak, Limits, RejectReason, Reservoir, SafetyInput,
};
use namo_core::telemetry::SensorState;

const CAL: Calibration = Calibration { ul_per_second: 6500 };
const MAX_PUMP_MS: u32 = 20_000;
const NOW: u64 = 1_772_500_000;

fn 정상_입력() -> SafetyInput {
    SafetyInput {
        now: Some(NOW),
        reservoir: Reservoir::Ok,
        leak: Leak::None,
        leak_updated_at: Some(NOW - 10),
        locked: false,
        running: false,
        last_completed_at: None,
        today_ml: 0,
        dose_ml: 100,
    }
}

fn 명령(id: &str, dose_ml: u32) -> WaterCommand {
    WaterCommand {
        id: CommandId::new(id).unwrap(),
        issued_at: NOW,
        ttl_s: 15,
        dose_ml,
    }
}

#[test]
fn 정상_급수_한_사이클() {
    let mut seen = RecentIds::new();
    let cmd = 명령("cmd-normal", 100);

    assert_eq!(check_ttl(&cmd, NOW + 2, 5), TtlVerdict::Fresh);
    assert!(seen.record(&cmd.id));

    let mut input = 정상_입력();
    input.dose_ml = cmd.dose_ml;
    assert_eq!(evaluate(&input, &Limits::DEFAULT), Ok(()));

    let p = plan(cmd.dose_ml, CAL, MAX_PUMP_MS).unwrap();
    assert!(!p.clamped);
    assert_eq!(estimated_ml(p.pump_ms, CAL), 100);

    // 구동 중 내내 정상이면 중단하지 않습니다.
    assert_eq!(abort_reason(Reservoir::Ok, Leak::None), None);
}

/// Stage 8 T8 — 같은 명령을 두 번 받아도 한 번만 실행합니다.
#[test]
fn 중복_명령은_한_번만_실행된다() {
    let mut seen = RecentIds::new();
    let cmd = 명령("cmd-dup", 100);

    assert!(seen.record(&cmd.id), "첫 수신은 통과해야 합니다");
    assert!(!seen.record(&cmd.id), "재수신은 막혀야 합니다");
}

/// Stage 8 T7 — 브로커 재연결로 뒤늦게 도착한 명령은 실행하지 않습니다.
#[test]
fn 만료된_명령은_실행되지_않는다() {
    let cmd = 명령("cmd-late", 100);
    assert_eq!(check_ttl(&cmd, NOW + 3_600, 5), TtlVerdict::Expired);
}

/// Stage 8 T1 — 빈 물통에서는 시작하지 않습니다.
#[test]
fn 빈_물통은_시작을_막는다() {
    let mut input = 정상_입력();
    input.reservoir = Reservoir::Empty;
    assert_eq!(
        evaluate(&input, &Limits::DEFAULT),
        Err(RejectReason::ReservoirEmpty)
    );
}

/// Stage 8 T2 — 구동 중 물통이 고갈되면 즉시 멈춥니다.
#[test]
fn 구동중_물통고갈은_즉시_멈춘다() {
    assert_eq!(
        abort_reason(Reservoir::Empty, Leak::None),
        Some(AbortReason::ReservoirEmpty)
    );
}

/// Stage 8 T3 — 구동 중 누수가 감지되면 즉시 멈춥니다.
#[test]
fn 구동중_누수는_즉시_멈춘다() {
    assert_eq!(
        abort_reason(Reservoir::Ok, Leak::Detected),
        Some(AbortReason::LeakDetected)
    );
}

/// Stage 8 T4 — 누수 잠금은 수동 해제 전까지 유지됩니다.
#[test]
fn 잠금은_누수가_사라져도_유지된다() {
    let mut input = 정상_입력();
    input.locked = true;
    input.leak = Leak::None; // 물기는 말랐지만
    assert_eq!(evaluate(&input, &Limits::DEFAULT), Err(RejectReason::Locked));
}

/// Stage 8 T5 — 급수 직후 재요청은 쿨다운으로 막힙니다.
#[test]
fn 급수_직후_재요청은_막힌다() {
    let mut input = 정상_입력();
    input.last_completed_at = Some(NOW - 5);
    assert_eq!(evaluate(&input, &Limits::DEFAULT), Err(RejectReason::Cooldown));
}

/// Stage 8 T6 — 일일 한도를 넘는 요청은 막힙니다.
#[test]
fn 일일한도_초과는_막힌다() {
    let mut input = 정상_입력();
    input.today_ml = 480;
    input.dose_ml = 50;
    assert_eq!(evaluate(&input, &Limits::DEFAULT), Err(RejectReason::DailyLimit));
}

/// Stage 8 T9 — 1회 상한을 넘는 요청은 클램프가 아니라 거부입니다.
#[test]
fn 과도한_요청은_클램프가_아니라_거부다() {
    let mut input = 정상_입력();
    input.dose_ml = 400;
    assert_eq!(
        evaluate(&input, &Limits::DEFAULT),
        Err(RejectReason::DoseTooLarge)
    );
}

/// Stage 8 T9b — 보정값이 틀려도 하드리밋이 최후 방어선입니다.
#[test]
fn 보정값이_틀려도_하드리밋이_막는다() {
    // 실제보다 10배 느리다고 잘못 보정된 상황.
    let 잘못된_보정 = Calibration { ul_per_second: 650 };
    let p = plan(300, 잘못된_보정, MAX_PUMP_MS).unwrap();
    assert_eq!(p.pump_ms, MAX_PUMP_MS);
    assert!(p.clamped);
}

/// Stage 7 — 게이트웨이가 죽으면 급수가 막힙니다(fail-closed).
#[test]
fn 게이트웨이_두절은_급수를_막는다() {
    let mut input = 정상_입력();
    input.leak_updated_at = Some(NOW - 600); // 10분간 갱신 없음
    assert_eq!(evaluate(&input, &Limits::DEFAULT), Err(RejectReason::LeakStale));
}

/// SNTP 동기 전에는 어떤 급수도 하지 않습니다.
#[test]
fn 시각_미동기_상태는_급수를_막는다() {
    let mut input = 정상_입력();
    input.now = None;
    assert_eq!(
        evaluate(&input, &Limits::DEFAULT),
        Err(RejectReason::ClockUnsynced)
    );
}

/// Stage 5 — 광고 여러 개를 모아야 센서 상태가 완성됩니다.
#[test]
fn 광고를_모아_센서상태를_완성한다() {
    const 수분: [u8; 16] = [
        0x71, 0x20, 0x98, 0x00, 0x0C, 0xEC, 0x9C, 0x8B, 0x6C, 0x7C, 0xC4, 0x00,
        0x08, 0x10, 0x01, 0x20,
    ];
    const 온도: [u8; 17] = [
        0x71, 0x20, 0x98, 0x00, 0x0D, 0xEC, 0x9C, 0x8B, 0x6C, 0x7C, 0xC4, 0x00,
        0x04, 0x10, 0x02, 0xE0, 0x00,
    ];
    const 조도: [u8; 18] = [
        0x71, 0x20, 0x98, 0x00, 0x0E, 0xEC, 0x9C, 0x8B, 0x6C, 0x7C, 0xC4, 0x00,
        0x07, 0x10, 0x03, 0x1C, 0x07, 0x00,
    ];
    const 전도도: [u8; 17] = [
        0x71, 0x20, 0x98, 0x00, 0x0F, 0xEC, 0x9C, 0x8B, 0x6C, 0x7C, 0xC4, 0x00,
        0x09, 0x10, 0x02, 0x54, 0x01,
    ];

    let mut state = SensorState::new();
    for (i, raw) in [수분.as_slice(), 온도.as_slice(), 조도.as_slice(), 전도도.as_slice()]
        .iter()
        .enumerate()
    {
        let (_, ms) = mibeacon::parse(raw).unwrap();
        state.apply_all(ms.as_slice(), NOW + i as u64);
    }

    assert!(state.is_complete());
    assert_eq!(state.moisture_pct, Some(32));
    assert_eq!(state.temperature_deci_c, Some(224));
    assert_eq!(state.illuminance_lux, Some(1820));
    assert_eq!(state.conductivity_us_cm, Some(340));
    assert_eq!(state.age_s(NOW + 10), Some(7));
}
