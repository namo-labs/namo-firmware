#!/usr/bin/env bash
#
# USB 웹캠을 HLS로 흘려보냅니다.
#
#   ./scripts/cam.sh          # 시작 (끊기면 알아서 다시 붙습니다)
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
# 640x480에 움직임이 거의 없는 화면이라 높게 잡을 이유가 없습니다.
# 올리면 화질보다 끊김이 먼저 옵니다.
BITRATE="${CAM_BITRATE:-500k}"
# 세그먼트가 이 시간 동안 갱신되지 않으면 얼어붙은 것으로 봅니다.
STALL_S="${CAM_STALL_S:-8}"

if ! command -v ffmpeg >/dev/null 2>&1; then
    echo "ffmpeg이 없습니다. brew install ffmpeg" >&2
    exit 1
fi

if [ "${1:-}" = "devices" ]; then
    ffmpeg -f avfoundation -list_devices true -i "" 2>&1 | grep -A10 'AVFoundation'
    exit 0
fi

rm -rf "$DIR"
mkdir -p "$DIR"

FF_PID=""
SRV_PID=""
RUNNING=1

cleanup() {
    RUNNING=0
    echo
    echo "정리 중..."
    kill "$FF_PID" "$SRV_PID" 2>/dev/null
    wait 2>/dev/null
    exit 0
}
trap cleanup INT TERM

stamp() { date '+%H:%M:%S'; }

start_ffmpeg() {
    # HLS 세그먼트를 만듭니다.
    #
    # 1초 세그먼트를 4개만 유지합니다. 길게 잡으면 지연이 그만큼 늘고,
    # 짧게 잡으면 요청이 잦아집니다. 식물을 보는 용도라 3~5초 지연은
    # 충분히 실시간으로 느껴집니다.
    #
    # 카메라는 4:2:2(uyvy422)로 주지만 출력은 yuv420p로 바꿉니다.
    # baseline 프로파일이 4:2:2를 받지 못하고, 브라우저도 4:2:0을
    # 기대합니다.
    #
    # maxrate와 bufsize로 상한을 겁니다. -b:v만 주면 목표치일 뿐이라
    # 화면이 바뀌는 순간 실제 비트레이트가 튀고, 그때 세그먼트가 커져
    # 재생이 끊깁니다.
    ffmpeg -hide_banner -loglevel warning \
        -f avfoundation -pixel_format uyvy422 \
        -framerate "$FPS" -video_size "$SIZE" \
        -i "${VIDEO_DEV}:${AUDIO_DEV}" \
        -r "$FPS" \
        -c:v libx264 -preset veryfast -tune zerolatency \
        -pix_fmt yuv420p -profile:v baseline \
        -g "$FPS" -keyint_min "$FPS" -sc_threshold 0 \
        -b:v "$BITRATE" -maxrate "$BITRATE" -bufsize "$BITRATE" \
        -c:a aac -b:a 64k -ar 44100 -ac 1 \
        -f hls -hls_time 1 -hls_list_size 4 \
        -hls_flags delete_segments+independent_segments+omit_endlist \
        -hls_segment_filename "$DIR/seg%05d.ts" \
        "$DIR/stream.m3u8" &
    FF_PID=$!
}

# 플레이리스트가 마지막으로 바뀐 뒤 흐른 시간(초).
since_update() {
    local mtime
    mtime=$(stat -f %m "$DIR/stream.m3u8" 2>/dev/null) || { echo 9999; return; }
    echo $(( $(date +%s) - mtime ))
}

echo "카메라 $VIDEO_DEV · 마이크 $AUDIO_DEV · ${SIZE}@${FPS}fps · ${BITRATE}"
echo "세그먼트: $DIR"
echo

# 세그먼트를 HTTP로 내보냅니다. 클러스터의 Caddy가 이 포트를 프록시합니다.
# ffmpeg과 달리 이쪽은 끊길 일이 없어 한 번만 띄웁니다.
(cd "$DIR" && python3 -m http.server "$PORT" --bind 0.0.0.0 >/dev/null 2>&1) &
SRV_PID=$!

start_ffmpeg

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

echo "[$(stamp)] 스트리밍 시작됨"
echo "  로컬 확인:  http://localhost:${PORT}/stream.m3u8"
echo "  멈추려면 Ctrl+C"
echo

# 감시 루프.
#
# 이 카메라는 프레임 공급을 멈추면서도 ffmpeg 프로세스는 살아 있는
# 상태로 얼어붙는 일이 있습니다. 프로세스 생사만 봐서는 못 잡으므로,
# **세그먼트가 실제로 갱신되는지**를 봅니다.
restarts=0
while [ "$RUNNING" = 1 ]; do
    sleep 2

    if ! kill -0 "$FF_PID" 2>/dev/null; then
        restarts=$((restarts + 1))
        echo "[$(stamp)] ffmpeg이 종료됨 — 다시 시작합니다 (${restarts}회)"
        start_ffmpeg
        sleep 3
        continue
    fi

    stalled=$(since_update)
    if [ "$stalled" -gt "$STALL_S" ]; then
        restarts=$((restarts + 1))
        echo "[$(stamp)] ${stalled}초째 새 세그먼트가 없음 — 다시 시작합니다 (${restarts}회)"
        kill "$FF_PID" 2>/dev/null
        wait "$FF_PID" 2>/dev/null
        # 장치를 놓을 시간을 줍니다. 곧바로 다시 열면 실패합니다.
        sleep 2
        start_ffmpeg
        sleep 3
    fi
done
