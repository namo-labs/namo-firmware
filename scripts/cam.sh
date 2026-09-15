#!/usr/bin/env bash
#
# USB 웹캠을 HLS로 흘려보냅니다.
#
#   ./scripts/cam.sh          # 시작
#   ./scripts/cam.sh devices  # 장치 번호 확인
#
# **맥 터미널에서 직접 실행해야 합니다.** macOS 카메라 권한은 프로세스를
# 띄운 앱에 붙는데, SSH나 자동화 도구로 띄우면 권한 요청 창을 띄울 수
# 없어 첫 프레임을 기다리며 조용히 멈춥니다. 처음 실행할 때 허용을
# 눌러두면 그 뒤로는 같은 터미널에서 계속 됩니다.

set -uo pipefail

VIDEO_DEV="${CAM_VIDEO:-0}"
AUDIO_DEV="${CAM_AUDIO:-1}"
PORT="${CAM_PORT:-8090}"
FPS="${CAM_FPS:-10}"
SIZE="${CAM_SIZE:-640x480}"
DIR="${CAM_DIR:-/tmp/namo-cam}"

if ! command -v ffmpeg >/dev/null 2>&1; then
    echo "ffmpeg이 없습니다. brew install ffmpeg" >&2
    exit 1
fi

if [ "${1:-}" = "devices" ]; then
    ffmpeg -f avfoundation -list_devices true -i "" 2>&1 | grep -A10 'AVFoundation'
    exit 0
fi

# 이전 실행이 남아 있으면 정리합니다. 세그먼트가 섞이면 재생이 깨집니다.
pkill -f "avfoundation.*${DIR}" 2>/dev/null
rm -rf "$DIR"
mkdir -p "$DIR"

cleanup() {
    echo
    echo "정리 중..."
    kill "${FF_PID:-}" "${SRV_PID:-}" 2>/dev/null
    wait 2>/dev/null
    exit 0
}
trap cleanup INT TERM

echo "카메라 $VIDEO_DEV · 마이크 $AUDIO_DEV · ${SIZE}@${FPS}fps"
echo "세그먼트: $DIR"
echo

# HLS 세그먼트를 만듭니다.
#
# 1초 세그먼트를 4개만 유지합니다. 길게 잡으면 지연이 그만큼 늘고,
# 짧게 잡으면 요청이 잦아집니다. 식물을 보는 용도라 3~5초 지연은
# 충분히 실시간으로 느껴집니다.
#
# -tune zerolatency 와 -g 는 키프레임을 매 세그먼트마다 넣어, 재생을
# 시작할 때 첫 그림이 늦게 뜨는 것을 막습니다.
#
# 카메라는 4:2:2(uyvy422)로 주지만 출력은 yuv420p 로 바꿉니다. baseline
# 프로파일이 4:2:2 를 받지 못하고, 브라우저도 4:2:0 을 기대합니다.
ffmpeg -hide_banner -loglevel warning \
    -f avfoundation -pixel_format uyvy422 \
    -framerate "$FPS" -video_size "$SIZE" \
    -i "${VIDEO_DEV}:${AUDIO_DEV}" \
    -r "$FPS" \
    -c:v libx264 -preset veryfast -tune zerolatency \
    -pix_fmt yuv420p -profile:v baseline \
    -g "$FPS" -keyint_min "$FPS" -sc_threshold 0 -b:v 900k \
    -c:a aac -b:a 64k -ar 44100 -ac 1 \
    -f hls -hls_time 1 -hls_list_size 4 \
    -hls_flags delete_segments+independent_segments+omit_endlist \
    -hls_segment_filename "$DIR/seg%05d.ts" \
    "$DIR/stream.m3u8" &
FF_PID=$!

# 세그먼트를 HTTP로 내보냅니다. 클러스터의 Caddy가 이 포트를 프록시합니다.
(cd "$DIR" && python3 -m http.server "$PORT" --bind 0.0.0.0 >/dev/null 2>&1) &
SRV_PID=$!

# 첫 세그먼트가 나올 때까지 기다립니다. 여기서 막히면 대개 권한입니다.
for _ in $(seq 1 30); do
    [ -f "$DIR/stream.m3u8" ] && break
    sleep 1
done

if [ ! -f "$DIR/stream.m3u8" ]; then
    echo "플레이리스트가 생기지 않았습니다." >&2
    echo "카메라 권한을 확인하세요: 시스템 설정 → 개인정보 보호 및 보안 → 카메라" >&2
    cleanup
fi

echo "스트리밍 시작됨"
echo "  로컬 확인:  http://localhost:${PORT}/stream.m3u8"
echo "  멈추려면 Ctrl+C"
echo

wait $FF_PID
echo "ffmpeg이 종료됐습니다." >&2
cleanup
