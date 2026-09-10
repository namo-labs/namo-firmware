#!/usr/bin/env bash
#
# Stage 8 — 안전규칙 통합 검증 (docs/bring-up-guide.md)
#
#   ./scripts/stage8.sh          # 자동 가능한 테스트를 순서대로
#   ./scripts/stage8.sh T7       # 특정 테스트만
#   ./scripts/stage8.sh list     # 목록
#
# ⚠️ 화분이 아니라 양동이에서 합니다. 일부러 실패시키는 단계입니다.

set -uo pipefail

BROKER="${BROKER_CONTAINER:-mosquitto}"
DEVICE_ID="${NAMO_DEVICE_ID:-pilot01}"
BASE="namo/pilot/$DEVICE_ID"
T_CMD="$BASE/water/cmd"
T_RESULT="$BASE/water/result"
T_UNLOCK="$BASE/unlock"
T_TELEMETRY="$BASE/telemetry"

PASS=0
FAIL=0

# ── 출력 ──────────────────────────────────────────────────────────
c_head() { printf '\n\033[1m── %s ─────────────────────────\033[0m\n' "$*"; }
c_pass() { printf '\033[32m  ✅ PASS\033[0m  %s\n' "$*"; PASS=$((PASS + 1)); }
c_fail() { printf '\033[31m  ❌ FAIL\033[0m  %s\n' "$*"; FAIL=$((FAIL + 1)); }
c_info() { printf '  %s\n' "$*"; }
c_ask()  { printf '\033[33m  ▶ %s\033[0m\n' "$*"; }

# 물리 조작이 필요한 지점에서 멈춥니다.
pause_for() {
    c_ask "$1"
    read -r -p "     준비되면 Enter (건너뛰려면 s + Enter): " ans
    [ "$ans" = "s" ] && return 1
    return 0
}

# ── MQTT ─────────────────────────────────────────────────────────
mqtt_pub() {
    docker exec "$BROKER" mosquitto_pub -h localhost -t "$1" -m "$2"
}

# 결과 토픽에서 메시지 하나를 기다립니다. 없으면 빈 문자열.
mqtt_wait_result() {
    local timeout="${1:-15}"
    docker exec "$BROKER" mosquitto_sub -h localhost -t "$T_RESULT" -C 1 -W "$timeout" 2>/dev/null
}

telemetry_field() {
    docker exec "$BROKER" mosquitto_sub -h localhost -t "$T_TELEMETRY" -C 1 -W 15 2>/dev/null \
        | jq -r ".$1 // empty"
}

now() { date +%s; }
new_id() { echo "t$(date +%s)$RANDOM"; }

# 명령을 보내고 결과를 받아옵니다. 결과 JSON을 stdout으로.
send_and_wait() {
    local id="$1" dose="$2" ttl="${3:-15}" issued="${4:-}"
    [ -z "$issued" ] && issued="$(now)"

    local out; out="$(mktemp)"
    docker exec "$BROKER" mosquitto_sub -h localhost -t "$T_RESULT" -C 1 -W 20 > "$out" 2>/dev/null &
    local sub=$!
    sleep 0.5
    mqtt_pub "$T_CMD" "{\"id\":\"$id\",\"issued_at\":$issued,\"ttl_s\":$ttl,\"dose_ml\":$dose}" >/dev/null
    wait $sub 2>/dev/null
    cat "$out"
    rm -f "$out"
}

# 결과 JSON이 기대한 status/reason인지 확인합니다.
expect() {
    local json="$1" want_status="$2" want_reason="${3:-}"

    if [ -z "$json" ]; then
        c_fail "결과 메시지가 오지 않았습니다 (기대: $want_status)"
        return 1
    fi

    local status reason
    status="$(echo "$json" | jq -r '.status // empty')"
    reason="$(echo "$json" | jq -r '.reason // "null"')"

    if [ "$status" != "$want_status" ]; then
        c_fail "status=$status (기대 $want_status), reason=$reason"
        return 1
    fi
    if [ -n "$want_reason" ] && [ "$reason" != "$want_reason" ]; then
        c_fail "reason=$reason (기대 $want_reason)"
        return 1
    fi
    c_pass "status=$status reason=$reason"
    return 0
}

# ── 사전 점검 ─────────────────────────────────────────────────────
preflight() {
    c_head "사전 점검"

    if ! docker ps --filter "name=$BROKER" --format '{{.Names}}' | grep -q "$BROKER"; then
        c_fail "브로커 컨테이너 '$BROKER'가 돌고 있지 않습니다"
        echo "     docker start $BROKER"
        exit 1
    fi
    c_pass "브로커 실행 중"

    local status
    status="$(docker exec "$BROKER" mosquitto_sub -h localhost -t "$BASE/status" -C 1 -W 5 2>/dev/null)"
    if [ "$status" != "online" ]; then
        c_fail "장치가 online이 아닙니다 (status=$status)"
        echo "     본 펌웨어가 올라가 있고 WiFi에 붙었는지 확인하세요."
        exit 1
    fi
    c_pass "장치 online"

    local leak; leak="$(telemetry_field leak)"
    if [ "$leak" = "unknown" ]; then
        c_fail "누수 상태가 unknown입니다. Zigbee2MQTT가 꺼져 있으면 모든 급수가 거부됩니다"
        echo "     export PATH=\"/opt/homebrew/opt/node@24/bin:\$PATH\"; cd ~/zigbee2mqtt && node index.js"
        exit 1
    fi
    c_pass "누수 상태: $leak"

    c_info "물통: $(telemetry_field reservoir) · 오늘 급수: $(telemetry_field today_estimated_ml)mL"
}

# ── 테스트 ────────────────────────────────────────────────────────

T1() {
    c_head "T1 — 빈 물통에서 급수 거부"
    pause_for "물통을 비우거나 플로트를 손으로 내려두세요." || return
    local r; r="$(telemetry_field reservoir)"
    if [ "$r" != "empty" ]; then
        c_fail "물통이 아직 $r 입니다. 플로트를 내려야 합니다"
        return
    fi
    expect "$(send_and_wait "$(new_id)" 100)" "rejected" "reservoir_empty"
}

T2() {
    c_head "T2 — 급수 중 물통 고갈 시 즉시 중단"
    pause_for "물통을 채우고, 급수가 시작되면 플로트를 손으로 내릴 준비를 하세요." || return
    c_info "명령을 보냅니다. 물이 나오기 시작하면 바로 플로트를 내리세요."
    expect "$(send_and_wait "$(new_id)" 300)" "aborted" "reservoir_empty"
}

T3() {
    c_head "T3 — 급수 중 누수 감지 시 중단 및 잠금"
    pause_for "물통을 채우고, 급수가 시작되면 누수센서를 적실 준비를 하세요." || return
    c_info "명령을 보냅니다. 물이 나오면 젖은 휴지를 센서 접점에 대세요."
    expect "$(send_and_wait "$(new_id)" 300)" "aborted" "leak_detected"

    sleep 2
    local locked; locked="$(telemetry_field locked)"
    if [ "$locked" = "true" ]; then
        c_pass "잠금 상태로 전환됨"
    else
        c_fail "잠기지 않았습니다 (locked=$locked)"
    fi
}

T4() {
    c_head "T4 — 누수가 사라져도 잠금 유지, unlock으로만 해제"
    pause_for "누수센서를 말리고 leak이 none으로 돌아올 때까지 기다리세요." || return

    local leak; leak="$(telemetry_field leak)"
    c_info "현재 누수 상태: $leak"

    expect "$(send_and_wait "$(new_id)" 100)" "rejected" "locked"

    c_info "unlock 명령을 보냅니다."
    mqtt_pub "$T_UNLOCK" "{\"id\":\"$(new_id)\"}" >/dev/null
    sleep 2

    local locked; locked="$(telemetry_field locked)"
    if [ "$locked" = "false" ]; then
        c_pass "unlock으로 잠금 해제됨"
    else
        c_fail "여전히 잠겨 있습니다 (locked=$locked)"
    fi
}

T5() {
    c_head "T5 — 급수 직후 쿨다운"
    pause_for "물통에 물이 있는지 확인하세요." || return

    c_info "1회차 급수 (100mL)"
    local first; first="$(send_and_wait "$(new_id)" 100)"
    if ! expect "$first" "completed"; then
        c_info "1회차가 성공해야 쿨다운을 검증할 수 있습니다."
        return
    fi

    c_info "곧바로 2회차 요청"
    expect "$(send_and_wait "$(new_id)" 100)" "rejected" "cooldown"
}

T6() {
    c_head "T6 — 일일 한도"
    c_info "한도(500mL)까지 채우려면 쿨다운 30분을 여러 번 기다려야 합니다."
    c_info "오늘 급수량: $(telemetry_field today_estimated_ml)mL"
    c_ask "이 테스트는 시간이 오래 걸려 수동으로 진행합니다."
    c_info "한도를 넘긴 뒤 요청하면 rejected(daily_limit)이 나와야 합니다."
}

T7() {
    c_head "T7 — TTL 만료 명령은 조용히 버려짐"
    local past=$(( $(now) - 300 ))
    c_info "issued_at을 5분 전으로 조작해 보냅니다. 결과가 오면 안 됩니다."

    local out; out="$(mktemp)"
    docker exec "$BROKER" mosquitto_sub -h localhost -t "$T_RESULT" -C 1 -W 10 > "$out" 2>/dev/null &
    local sub=$!
    sleep 0.5
    mqtt_pub "$T_CMD" "{\"id\":\"$(new_id)\",\"issued_at\":$past,\"ttl_s\":15,\"dose_ml\":100}" >/dev/null
    wait $sub 2>/dev/null

    if [ -s "$out" ]; then
        c_fail "결과가 발행됐습니다: $(cat "$out")"
    else
        c_pass "결과 없음 (설계대로 조용히 버림)"
    fi
    rm -f "$out"
}

T8() {
    c_head "T8 — 같은 ID 재수신 시 재실행 안 함"
    pause_for "물통에 물이 있고 쿨다운이 끝났는지 확인하세요." || return

    local id; id="$(new_id)"
    c_info "1회차 (id=$id)"
    local first; first="$(send_and_wait "$id" 100)"
    local status; status="$(echo "$first" | jq -r '.status // empty')"
    c_info "1회차 결과: $status"

    c_info "같은 id로 2회차"
    local out; out="$(mktemp)"
    docker exec "$BROKER" mosquitto_sub -h localhost -t "$T_RESULT" -C 1 -W 10 > "$out" 2>/dev/null &
    local sub=$!
    sleep 0.5
    mqtt_pub "$T_CMD" "{\"id\":\"$id\",\"issued_at\":$(now),\"ttl_s\":15,\"dose_ml\":100}" >/dev/null
    wait $sub 2>/dev/null

    if [ -s "$out" ]; then
        c_fail "두 번째 명령에 결과가 왔습니다: $(cat "$out")"
    else
        c_pass "두 번째는 무시됨"
    fi
    rm -f "$out"
}

T9() {
    c_head "T9 — 1회 급수량 상한 초과 거부"
    c_info "400mL를 요청합니다 (상한 300mL)."
    expect "$(send_and_wait "$(new_id)" 400)" "rejected" "dose_too_large"
}

T9b() {
    c_head "T9b — 시간 하드리밋"
    c_info "보정값을 일부러 낮춘 빌드가 필요해 스크립트로 자동화하지 않습니다."
    c_info "config.rs의 CALIBRATION을 1/10로 낮추고 300mL를 요청하면,"
    c_info "MAX_PUMP_MS(10초)에서 강제 정지하고 로그에 하드리밋 기록이 남아야 합니다."
    c_ask "수동으로 진행하세요."
}

T10() {
    c_head "T10 — WiFi가 끊겨도 펌프는 정확히 종료"
    c_info "급수 시작 직후 공유기를 끄거나 보드를 WiFi에서 떼어냅니다."
    c_info "펌프는 로컬 타이머로 계획한 시간에 정확히 꺼져야 합니다(S11)."
    c_ask "수동으로 진행하세요. 결과 메시지는 오지 않는 것이 정상입니다."
}

T11() {
    c_head "T11 — 브로커가 죽어도 장치는 살아 있음"
    pause_for "브로커를 잠시 멈춥니다. 계속할까요?" || return

    docker stop "$BROKER" >/dev/null
    c_info "브로커 중지. 10초 대기..."
    sleep 10
    docker start "$BROKER" >/dev/null
    c_info "브로커 재시작. 장치가 다시 붙는지 확인합니다..."
    sleep 15

    local status
    status="$(docker exec "$BROKER" mosquitto_sub -h localhost -t "$BASE/status" -C 1 -W 30 2>/dev/null)"
    if [ "$status" = "online" ]; then
        c_pass "장치가 스스로 재접속했습니다"
    else
        c_fail "재접속 실패 (status=$status)"
    fi
}

T12() {
    c_head "T12 — 급수 중 리셋"
    c_info "급수 중 보드의 리셋 버튼을 누릅니다."
    c_info "펌프가 즉시 멈추고, 재부팅 후에도 돌지 않아야 합니다."
    c_ask "수동으로 진행하세요."
}

T13() {
    c_head "T13 — 전원 재투입 시 펌프 무동작"
    c_info "ESP32 USB를 10회 뽑았다 꽂습니다."
    c_info "펌프가 한 번도 돌지 않아야 합니다. 한 번이라도 돌면 게이트 풀다운을 확인하세요."
    c_ask "수동으로 진행하세요. 이 테스트가 S1·S2의 최종 확인입니다."
}

# ── 실행 ──────────────────────────────────────────────────────────
ALL=(T1 T2 T3 T4 T5 T6 T7 T8 T9 T9b T10 T11 T12 T13)

if [ $# -gt 0 ]; then
    case "$1" in
        list)
            printf '%s\n' "${ALL[@]}"
            exit 0
            ;;
        *)
            preflight
            "$1"
            ;;
    esac
else
    preflight
    for t in "${ALL[@]}"; do "$t"; done
fi

c_head "결과"
printf '  통과 %d · 실패 %d\n' "$PASS" "$FAIL"
if [ "$FAIL" -gt 0 ]; then
    printf '\033[31m  실패가 있습니다. 화분에 연결하지 마세요.\033[0m\n'
    exit 1
fi
