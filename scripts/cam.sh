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

# 장치는 **이름으로** 지정합니다. 번호는 아이폰이 붙었다 떨어질 때마다
# 밀려서, 어제 되던 명령이 오늘 "Invalid audio device index"로 죽습니다.
VIDEO_DEV="${CAM_VIDEO:-USB2.0 PC CAMERA}"
# 소리가 필요 없으면 CAM_AUDIO=none 으로 끕니다. 마이크가 흔들리면
# 영상까지 끌려가 멈추는데, 소리를 끄면 그 연결이 끊깁니다.
AUDIO_DEV="${CAM_AUDIO:-USB2.0 MIC}"
PORT="${CAM_PORT:-8090}"
FPS="${CAM_FPS:-10}"
SIZE="${CAM_SIZE:-640x480}"
DIR="${CAM_DIR:-/tmp/namo-cam}"
# ffmpeg이 뱉는 것은 파일로 받습니다. 터미널로 흘리면 재시작이 잦을 때
# 정작 봐야 할 첫 줄이 위로 밀려 사라집니다.
LOG="$DIR/ffmpeg.log"
# 스트림 서버의 출력은 따로 받습니다. 접속 기록이 요청마다 한 줄씩 쌓여,
# 같은 파일에 두면 정작 ffmpeg 오류가 묻힙니다.
SRV_LOG="$DIR/server.log"
# 640x480에 움직임이 거의 없는 화면이라 높게 잡을 이유가 없습니다.
# 올리면 화질보다 끊김이 먼저 옵니다.
BITRATE="${CAM_BITRATE:-500k}"
# 세그먼트가 이 시간 동안 갱신되지 않으면 얼어붙은 것으로 봅니다.
STALL_S="${CAM_STALL_S:-8}"
# 새로 띄운 ffmpeg에 첫 세그먼트를 기다려주는 시간. 이 동안은 멈춤을
# 판정하지 않습니다.
#
# 장치를 여는 데 몇 초, muxer가 오디오와 영상을 맞추느라 또 몇 초가
# 걸립니다. 이것을 STALL_S로 재면 첫 세그먼트가 나오기 직전에 죽이고,
# 죽이면서 쌓인 것을 쏟게 하고, 다시 띄우는 일을 끝없이 되풀이합니다.
# 실제로 14시간을 10초마다 그랬습니다.
START_WAIT_S="${CAM_START_WAIT_S:-30}"

if ! command -v ffmpeg >/dev/null 2>&1; then
    echo "ffmpeg이 없습니다. brew install ffmpeg" >&2
    exit 1
fi

if [ "${1:-}" = "devices" ]; then
    ffmpeg -f avfoundation -list_devices true -i "" 2>&1 | grep -A10 'AVFoundation'
    exit 0
fi

# 디렉토리는 남기고 안만 비웁니다. 통째로 지우면 이전 실행이 남긴
# 서버가 사라진 디렉토리를 붙잡은 채로 남습니다.
mkdir -p "$DIR"
rm -f "$DIR"/*.ts "$DIR"/*.m3u8 "$DIR"/*.jpg "$DIR"/*.log

FF_PID=""
SRV_PID=""
RUNNING=1

cleanup() {
    RUNNING=0
    echo
    echo "정리 중..."
    stop_ffmpeg
    kill "$SRV_PID" 2>/dev/null
    wait 2>/dev/null
    exit 0
}
trap cleanup INT TERM

stamp() { date '+%H:%M:%S'; }

# ffmpeg을 확실히 내립니다.
#
# **여기에 wait만 쓰면 안 됩니다.** avfoundation 입력에서 막힌 ffmpeg은
# SIGTERM을 처리할 기회조차 없어 그대로 남고, wait은 영원히 돌아오지
# 않습니다. 그러면 되살리라고 만든 감시 루프가 첫 재시작에서 멈춰버려,
# 카메라가 죽은 채로 몇 시간이 지납니다. 실제로 6시간을 그랬습니다.
stop_ffmpeg() {
    [ -n "$FF_PID" ] || return 0
    kill "$FF_PID" 2>/dev/null

    local i
    for i in 1 2 3 4 5; do
        kill -0 "$FF_PID" 2>/dev/null || break
        sleep 1
    done

    if kill -0 "$FF_PID" 2>/dev/null; then
        echo "[$(stamp)] ffmpeg이 SIGTERM에 응하지 않아 강제로 내립니다"
        kill -9 "$FF_PID" 2>/dev/null
    fi
    # 여기서는 이미 죽은 뒤라 곧바로 돌아옵니다. 좀비를 거두기 위한
    # 것입니다.
    wait "$FF_PID" 2>/dev/null
    FF_PID=""
}

# 소리를 끄면 인코딩 인자도 뺍니다.
if [ "$AUDIO_DEV" = "none" ]; then
    AUDIO_ARGS=(-an)
else
    AUDIO_ARGS=(-c:a aac -b:a 64k -ar 44100 -ac 1)
fi

start_ffmpeg() {
    # HLS 세그먼트를 만듭니다.
    #
    # 1초 세그먼트를 10개 유지합니다.
    #
    # 이 마이크는 소리를 5초마다 몰아서 넘기고, muxer는 그 소리를 기다렸다
    # 영상과 맞춰 쓰므로 세그먼트도 5초마다 5개씩 한꺼번에 나옵니다. 4개만
    # 유지하면 한 묶음이 들어오는 순간 앞의 것이 지워져, 브라우저가 받기도
    # 전에 404가 납니다. 한 묶음보다 넉넉해야 합니다.
    #
    # 카메라는 4:2:2(uyvy422)로 주지만 출력은 yuv420p로 바꿉니다.
    # baseline 프로파일이 4:2:2를 받지 못하고, 브라우저도 4:2:0을
    # 기대합니다.
    #
    # maxrate와 bufsize로 상한을 겁니다. -b:v만 주면 목표치일 뿐이라
    # 화면이 바뀌는 순간 실제 비트레이트가 튀고, 그때 세그먼트가 커져
    # 재생이 끊깁니다.
    #
    # muxer는 늦게 오는 소리를 기다렸다가 영상과 시간순으로 섞어 씁니다.
    # **이 기다림을 줄이면 안 됩니다.** 기다리지 않으면 늦은 소리가 엉뚱한
    # 세그먼트에 몰려 들어가고, 브라우저는 시간이 어긋난 세그먼트를
    # 받아들이지 못해(bufferAppendError) 멈춥니다. 한도를 2초로 줄였다가
    # 실제로 그랬습니다. 기다리는 동안 파일이 안 나오는 것은 감시 쪽이
    # START_WAIT_S로 견딥니다.
    #
    # HLS와 함께 5초마다 JPEG 한 장을 덮어씁니다. 앱 목록이나 알림에
    # 붙일 그림은 플레이어가 필요 없는 편이 낫습니다. 이미 디코딩한
    # 프레임을 쓰므로 부담이 거의 없습니다. atomic_writing 을 켜야
    # 쓰는 도중에 읽어 깨진 그림이 나가지 않습니다.
    # **-y가 없으면 안 됩니다.** 재시작할 때는 snapshot.jpg가 이미
    # 있는데, 그러면 ffmpeg이 덮어쓸지 되묻고 답이 없어 죽습니다.
    # 감시 루프는 그걸 다시 띄우고, 또 되묻고, 또 죽습니다. 실제로
    # 84번을 그랬습니다.
    ffmpeg -hide_banner -loglevel warning -y \
        -f avfoundation -pixel_format uyvy422 \
        -framerate "$FPS" -video_size "$SIZE" \
        -i "${VIDEO_DEV}:${AUDIO_DEV}" \
        -r "$FPS" \
        -c:v libx264 -preset veryfast -tune zerolatency \
        -pix_fmt yuv420p -profile:v baseline \
        -g "$FPS" -keyint_min "$FPS" -sc_threshold 0 \
        -b:v "$BITRATE" -maxrate "$BITRATE" -bufsize "$BITRATE" \
        "${AUDIO_ARGS[@]}" \
        -f hls -hls_time 1 -hls_list_size 10 \
        -hls_flags delete_segments+independent_segments+omit_endlist \
        -hls_segment_filename "$DIR/seg%05d.ts" \
        "$DIR/stream.m3u8" \
        -map 0:v -vf fps=1/5 -update 1 -q:v 4 -atomic_writing 1 \
        "$DIR/snapshot.jpg" 2>>"$LOG" &
    FF_PID=$!
}

# 플레이리스트의 미디어 시퀀스. 세그먼트가 하나 나올 때마다 늘어납니다.
current_seq() {
    sed -n 's/^#EXT-X-MEDIA-SEQUENCE:\([0-9][0-9]*\).*/\1/p' \
        "$DIR/stream.m3u8" 2>/dev/null | head -1
}

# 새 세그먼트가 나온 지 흐른 시간을 STALLED에, 이번에 번호가 늘었는지를
# ADVANCED에 넣습니다.
#
# **파일의 mtime을 보면 안 됩니다.** ffmpeg은 종료할 때도 플레이리스트를
# 다시 쓰기 때문에, 재시작을 반복하는 동안 파일은 계속 새것처럼 보입니다.
# 정작 프레임은 한 장도 안 나오는데 말입니다. 그러면 멈춘 것을 멈췄다고
# 부르지 못하고, 재시작 횟수도 셀 수 없습니다.
#
# 값을 표준출력으로 돌려주지 않는 것은 $(...)가 서브셸이라 last_seq 갱신이
# 사라지기 때문입니다.
STALLED=0
ADVANCED=0
last_seq=""
last_seq_at=$(date +%s)

update_stall() {
    local seq now
    seq=$(current_seq)
    now=$(date +%s)

    ADVANCED=0
    if [ -n "$seq" ]; then
        if [ -z "$last_seq" ] || [ "$seq" -gt "$last_seq" ]; then
            last_seq_at="$now"
            ADVANCED=1
        fi
        # 줄어든 것은 ffmpeg이 새로 떠서 0부터 다시 센 것입니다. 기준만
        # 옮기고 진행으로 세지 않습니다. 바뀌었다는 이유만으로 세면,
        # 죽을 때마다 쏟아낸 세그먼트를 살아난 것으로 착각합니다.
        last_seq="$seq"
    fi
    STALLED=$(( now - last_seq_at ))
}

# 재시작 뒤 첫 세그먼트를 기다리는 동안이면 참입니다.
GRACE_UNTIL=0
in_grace() {
    [ "$(date +%s)" -lt "$GRACE_UNTIL" ]
}

# 이름이 실제로 있는지 먼저 봅니다. 없는 채로 들어가면 ffmpeg이
# 알아보기 어려운 오류를 내고 죽습니다.
DEVICES="$(ffmpeg -f avfoundation -list_devices true -i "" 2>&1)"
for d in "$VIDEO_DEV" "$AUDIO_DEV"; do
    [ "$d" = "none" ] && continue
    if ! grep -qF "$d" <<<"$DEVICES"; then
        echo "장치를 찾지 못했습니다: $d" >&2
        echo >&2
        echo "$DEVICES" | grep -E '^\[AVFoundation.*\] (\[|AVFoundation)' >&2
        echo >&2
        echo "CAM_VIDEO / CAM_AUDIO 로 이름을 지정할 수 있습니다." >&2
        exit 1
    fi
done

echo "카메라 $VIDEO_DEV · 마이크 $AUDIO_DEV · ${SIZE}@${FPS}fps · ${BITRATE}"
echo "세그먼트: $DIR"
echo

# 포트를 이미 누가 듣고 있으면 여기서 멈춥니다.
#
# 예전 실행이 남긴 서버는 터미널을 닫아도 살아남아 고아가 됩니다.
# 그대로 두고 새로 띄우면 이쪽이 "Address already in use"로 조용히
# 죽고(출력을 버리므로 알 수도 없습니다), 옛 서버가 계속 내보냅니다.
if lsof -nP -iTCP:"$PORT" -sTCP:LISTEN >/dev/null 2>&1; then
    echo "포트 $PORT 를 이미 누가 듣고 있습니다:" >&2
    lsof -nP -iTCP:"$PORT" -sTCP:LISTEN >&2
    echo >&2
    echo "예전 실행이 남긴 것이면 정리한 뒤 다시 실행하세요:" >&2
    echo "  lsof -ti tcp:$PORT | xargs kill" >&2
    exit 1
fi

# 세그먼트를 HTTP로 내보냅니다. 클러스터의 Caddy가 이 포트를 프록시합니다.
# ffmpeg과 달리 이쪽은 끊길 일이 없어 한 번만 띄웁니다.
(cd "$DIR" && python3 -m http.server "$PORT" --bind 0.0.0.0 >>"$SRV_LOG" 2>&1) &
SRV_PID=$!

sleep 1
if ! kill -0 "$SRV_PID" 2>/dev/null; then
    echo "스트림 서버를 띄우지 못했습니다. 로그: $SRV_LOG" >&2
    tail -3 "$SRV_LOG" >&2
    exit 1
fi

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

last_seq_at=$(date +%s)

echo "[$(stamp)] 스트리밍 시작됨"
echo "  로컬 확인:  http://localhost:${PORT}/stream.m3u8"
echo "  스냅샷:     http://localhost:${PORT}/snapshot.jpg"
echo "  ffmpeg 로그: $LOG"
echo "  멈추려면 Ctrl+C"
echo

# 감시 루프.
#
# 이 카메라는 프레임 공급을 멈추면서도 ffmpeg 프로세스는 살아 있는
# 상태로 얼어붙는 일이 있습니다. 프로세스 생사만 봐서는 못 잡으므로,
# **세그먼트가 실제로 갱신되는지**를 봅니다.
restarts=0
# 재시작하고도 프레임이 돌아오지 않은 횟수.
failed=0
# ffmpeg이 뜨자마자 죽은 횟수.
died=0
while [ "$RUNNING" = 1 ]; do
    sleep 2

    if ! kill -0 "$FF_PID" 2>/dev/null; then
        restarts=$((restarts + 1))
        echo "[$(stamp)] ffmpeg이 종료됨 — 다시 시작합니다 (${restarts}회)"
        wait "$FF_PID" 2>/dev/null

        # 뜨자마자 죽는 것은 몇 번을 더 띄워도 풀리지 않습니다.
        # 명령줄이나 장치가 문제이므로 오류를 보여주고 사람을 부릅니다.
        died=$((died + 1))
        if [ "$died" -ge 3 ]; then
            echo "[$(stamp)] ffmpeg이 뜨자마자 ${died}번 죽었습니다. 마지막 오류:"
            tail -4 "$LOG" 2>/dev/null | sed 's/^/           /'
            echo "           전체 로그: $LOG"
            died=0
        fi

        start_ffmpeg
        GRACE_UNTIL=$(( $(date +%s) + START_WAIT_S ))
        sleep 3
        continue
    fi

    # 여기까지 왔다는 것은 3초를 넘겨 살아 있다는 뜻입니다.
    died=0

    update_stall
    if [ "$ADVANCED" = 1 ]; then
        # **프레임이 실제로 나왔을 때만** 실패 횟수를 지웁니다. 재시작했다는
        # 이유로 지우면 영원히 1을 넘지 못해, 사람을 부를 일이 없어집니다.
        if [ "$failed" -gt 0 ]; then
            echo "[$(stamp)] 세그먼트가 다시 나옵니다"
        fi
        failed=0
        GRACE_UNTIL=0
    fi

    # 새로 띄운 ffmpeg은 첫 세그먼트를 낼 때까지 건드리지 않습니다.
    if in_grace; then
        continue
    fi

    if [ "$STALLED" -gt "$STALL_S" ]; then
        restarts=$((restarts + 1))
        failed=$((failed + 1))
        echo "[$(stamp)] ${STALLED}초째 새 세그먼트가 없음 — 다시 시작합니다 (${restarts}회)"
        stop_ffmpeg

        # 실패가 이어지면 간격을 늘립니다. 카메라가 안 돌아오는데 10초마다
        # USB 장치를 여닫으면 장치를 더 괴롭힐 뿐입니다.
        backoff=2
        if [ "$failed" -ge 3 ]; then
            backoff=$(( failed * 10 ))
            [ "$backoff" -gt 120 ] && backoff=120
            echo "           ${backoff}초 쉬었다 띄웁니다"
        fi
        sleep "$backoff"

        start_ffmpeg
        GRACE_UNTIL=$(( $(date +%s) + START_WAIT_S ))
        last_seq_at=$(date +%s)

        # 재시작해도 프레임이 돌아오지 않으면 카메라 쪽 문제입니다.
        # ffmpeg을 몇 번 더 띄운다고 풀리지 않으므로 사람을 부릅니다.
        if [ "$failed" -eq 5 ]; then
            echo "[$(stamp)] 재시작 ${failed}회째 프레임이 돌아오지 않습니다."
            echo "           웹캠 USB를 뽑았다 꽂아보세요."
            echo "           다른 ffmpeg이 카메라를 붙잡고 있을 수도 있습니다:"
            echo "           pgrep -fl ffmpeg"
            echo "           ffmpeg 로그: $LOG"
        fi
    fi
done
