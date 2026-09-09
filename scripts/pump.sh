#!/usr/bin/env bash
#
# 펌프를 원하는 시간만큼 한 번 돌립니다. bring-up 중 손으로 시험할 때 씁니다.
#
#   ./scripts/pump.sh          # 기본 2초
#   ./scripts/pump.sh 5        # 5초
#   ./scripts/pump.sh 5 20     # 5초, 카운트다운 20초 (12V를 나중에 꽂을 때)
#
# 안전상 20초를 넘겨 요청해도 드라이버가 잘라냅니다(S3a).

set -euo pipefail

RUN_S="${1:-2}"
COUNTDOWN_S="${2:-5}"

if ! [[ "$RUN_S" =~ ^[0-9]+$ ]] || [ "$RUN_S" -lt 1 ]; then
    echo "구동 시간은 1 이상의 정수(초)여야 합니다: $RUN_S" >&2
    exit 1
fi

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CRATE="$REPO_ROOT/crates/namo-firmware"

export PATH="$HOME/.cargo/bin:$PATH"

# 보드 포트를 찾습니다. Zigbee 동글(usbserial)과 헷갈리지 않게 usbmodem만 봅니다.
PORT="$(ls /dev/cu.usbmodem* 2>/dev/null | head -1 || true)"
if [ -z "$PORT" ]; then
    echo "ESP32 보드를 찾지 못했습니다. USB 연결을 확인하세요." >&2
    exit 1
fi

echo "포트: $PORT"
echo "구동: ${RUN_S}초 (카운트다운 ${COUNTDOWN_S}초)"
echo

cd "$CRATE"
PUMP_RUN_MS=$((RUN_S * 1000)) PUMP_COUNTDOWN_S="$COUNTDOWN_S" \
    cargo build --release --bin pumprun

if ! espflash flash --port "$PORT" --monitor \
    target/xtensa-esp32s3-espidf/release/pumprun; then
    echo >&2
    echo "플래싱에 실패했습니다." >&2
    echo "'Device or resource busy'라면 다른 프로그램이 포트를 잡고 있는 것입니다." >&2
    echo "다른 터미널의 espflash monitor나 시리얼 모니터를 닫고 다시 시도하세요." >&2
    exit 1
fi
