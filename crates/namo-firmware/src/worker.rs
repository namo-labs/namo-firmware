//! 급수 워커. 스펙 `docs/pilot-design.md` §4.6.
//!
//! 명령 하나의 수명주기가 전부 여기 있습니다. 안전 판정은 `namo-core`의
//! 순수 함수에 맡기고, 이 모듈은 순서와 부작용만 책임집니다.

use namo_core::command::{check_ttl, TtlVerdict, WaterCommand};
use namo_core::daily::KST_OFFSET_S;
use namo_core::dose;
use namo_core::safety::{self, AbortReason};
use serde::Serialize;
use std::sync::mpsc::Receiver;

use crate::clock;
use crate::config::{CALIBRATION, LIMITS, MAX_FUTURE_SKEW_S, MAX_PUMP_MS};
use crate::hw::pump::Pump;
use crate::net::mqtt::{Publisher, Topics};
use crate::state::{PumpState, Shared};

/// 결과 페이로드 (§5.4).
#[derive(Debug, Serialize)]
struct WaterResult<'a> {
    id: &'a str,
    status: &'static str,
    reason: Option<&'static str>,
    estimated_ml: u32,
    pump_ms: u32,
    started_at: Option<u64>,
    finished_at: Option<u64>,
}

/// 명령을 하나씩 처리합니다. 돌아오지 않습니다.
///
/// 한 번에 하나만 처리하므로 동시 급수가 구조적으로 불가능합니다. 큐에 쌓인
/// 명령은 앞의 급수가 끝난 뒤에 판정되며, 그 시점에 TTL이 지났으면 버려집니다.
pub fn run(
    commands: Receiver<WaterCommand>,
    mut pump: Pump<'static>,
    shared: Shared,
    publisher: Publisher,
    topics: std::sync::Arc<Topics>,
) {
    while let Ok(command) = commands.recv() {
        handle(&command, &mut pump, &shared, &publisher, &topics);
    }
    log::error!("명령 채널이 닫혔습니다. 급수 워커를 종료합니다.");
}

fn handle(
    command: &WaterCommand,
    pump: &mut Pump<'static>,
    shared: &Shared,
    publisher: &Publisher,
    topics: &Topics,
) {
    let id = command.id.as_str();

    // ── S8: TTL ───────────────────────────────────────────────────
    // 시계를 모르면 TTL을 잴 수 없습니다. 그 경우는 아래 안전 판정이
    // clock_unsynced로 거부하므로, 여기서는 시각이 있을 때만 검사합니다.
    if let Some(now) = clock::now() {
        match check_ttl(command, now, MAX_FUTURE_SKEW_S) {
            TtlVerdict::Expired => {
                // 결과를 발행하지 않습니다(§4.6). 뒤늦게 도착한 명령에
                // 답을 주면 그걸 보고 재시도하는 흐름이 생깁니다.
                log::warn!("명령 {id}: TTL 만료. 조용히 버립니다.");
                return;
            }
            TtlVerdict::Fresh => {}
        }
    }

    // ── S9: 멱등성 ────────────────────────────────────────────────
    // 이미 처리한 ID면 아무 것도 하지 않습니다. 결과도 다시 보내지
    // 않습니다. 첫 처리 때 이미 보냈기 때문입니다.
    //
    // 기록은 안전 판정보다 **앞**에 둡니다. 거부된 명령의 ID도 소모되므로,
    // 물통을 채운 뒤 같은 ID로 다시 보내면 무시됩니다. 재시도는 새 ID로
    // 해야 합니다. 판정 뒤에 기록하면 거부는 재시도할 수 있게 되지만,
    // 그러면 "브로커가 같은 명령을 두 번 전달했다"와 "사용자가 다시
    // 눌렀다"를 ID만으로 구분할 수 없게 됩니다. 물을 두 번 주는 쪽이
    // 더 나쁘므로 보수적인 쪽을 택합니다.
    {
        let Ok(mut state) = shared.lock() else {
            log::error!("상태 락이 깨졌습니다");
            return;
        };
        if !state.recent_ids.record(&command.id) {
            log::warn!("명령 {id}: 이미 처리한 ID입니다. 무시합니다.");
            return;
        }
    }

    // ── 안전 판정 ─────────────────────────────────────────────────
    let now = clock::now();
    let input = {
        let Ok(state) = shared.lock() else {
            return;
        };
        state.safety_input(now, command.dose_ml)
    };

    if let Err(reason) = safety::evaluate(&input, &LIMITS) {
        log::warn!("명령 {id}: 거부 ({})", reason.as_str());
        publish(
            publisher,
            topics,
            WaterResult {
                id,
                status: "rejected",
                reason: Some(reason.as_str()),
                estimated_ml: 0,
                pump_ms: 0,
                started_at: now,
                finished_at: now,
            },
        );
        return;
    }

    // ── 구동시간 계산 ─────────────────────────────────────────────
    let plan = match dose::plan(command.dose_ml, CALIBRATION, MAX_PUMP_MS) {
        Ok(p) => p,
        Err(e) => {
            log::error!("명령 {id}: 구동시간 계산 실패 ({e:?})");
            publish(
                publisher,
                topics,
                WaterResult {
                    id,
                    status: "failed",
                    reason: Some("hardware_error"),
                    estimated_ml: 0,
                    pump_ms: 0,
                    started_at: now,
                    finished_at: clock::now(),
                },
            );
            return;
        }
    };

    if plan.clamped {
        log::warn!(
            "명령 {id}: {}mL 요청이 하드리밋에 걸려 {}ms로 잘렸습니다. 보정값을 확인하세요.",
            command.dose_ml,
            plan.pump_ms
        );
    }

    // ── 구동 ──────────────────────────────────────────────────────
    set_pump_state(shared, PumpState::Running);
    let started_at = clock::now();
    log::info!("명령 {id}: 급수 시작 ({}ms)", plan.pump_ms);

    let outcome = pump.run(plan.pump_ms, || abort_check(shared));

    set_pump_state(shared, PumpState::Idle);
    let finished_at = clock::now();

    let outcome = match outcome {
        Ok(o) => o,
        Err(e) => {
            // 펌프는 드라이버가 이미 내렸습니다. 여기서는 보고만 합니다.
            log::error!("명령 {id}: 펌프 구동 실패 ({e})");
            publish(
                publisher,
                topics,
                WaterResult {
                    id,
                    status: "failed",
                    reason: Some("hardware_error"),
                    estimated_ml: 0,
                    pump_ms: 0,
                    started_at,
                    finished_at,
                },
            );
            return;
        }
    };

    // ── 결과 정리 ─────────────────────────────────────────────────
    // 급수량은 계획이 아니라 **실제로 돈 시간**으로 추정합니다. 중단된
    // 경우 계획대로 적으면 실제보다 많이 준 것으로 기록됩니다.
    let estimated_ml = dose::estimated_ml(outcome.actual_ms, CALIBRATION);

    if let Ok(mut state) = shared.lock() {
        if let Some(t) = finished_at {
            state.daily.add(estimated_ml, t, KST_OFFSET_S);
            // 쿨다운(S6)은 중단된 급수에도 겁니다. 물이 조금이라도 나갔고,
            // 중단 직후 바로 재시도하는 흐름을 막아야 합니다.
            state.last_completed_at = Some(t);
        }
    }

    let (status, reason) = match outcome.aborted {
        Some(r) => ("aborted", Some(r.as_str())),
        None => ("completed", None),
    };

    log::info!(
        "명령 {id}: {status} ({}ms, 약 {estimated_ml}mL)",
        outcome.actual_ms
    );

    publish(
        publisher,
        topics,
        WaterResult {
            id,
            status,
            reason,
            estimated_ml,
            pump_ms: outcome.actual_ms,
            started_at,
            finished_at,
        },
    );
}

/// 구동 중 중단조건 확인 (S4·S5). 펌프 드라이버가 주기적으로 호출합니다.
///
/// 락을 아주 짧게만 잡습니다. 여기서 오래 붙들면 그동안 펌프가 계속 돕니다.
fn abort_check(shared: &Shared) -> Option<AbortReason> {
    let Ok(state) = shared.lock() else {
        // 락이 깨졌으면 상태를 믿을 수 없습니다. 멈추는 쪽을 택합니다.
        log::error!("상태 락이 깨졌습니다. 급수를 중단합니다.");
        return Some(AbortReason::ReservoirEmpty);
    };
    safety::abort_reason(state.reservoir, state.leak())
}

fn set_pump_state(shared: &Shared, next: PumpState) {
    if let Ok(mut state) = shared.lock() {
        state.pump = next;
    }
}

fn publish(publisher: &Publisher, topics: &Topics, result: WaterResult<'_>) {
    match serde_json::to_vec(&result) {
        Ok(payload) => publisher.publish(&topics.water_result, &payload, false),
        Err(e) => log::error!("결과 직렬화 실패: {e}"),
    }
}
