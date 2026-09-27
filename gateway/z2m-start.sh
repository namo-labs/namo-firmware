#!/usr/bin/env bash
#
# Zigbee2MQTT를 띄웁니다. LaunchAgent가 이 스크립트를 부릅니다.
#
# 동글의 포트 이름(/dev/cu.usbserial-XXX)은 꽂힌 위치로 정해집니다. 허브를
# 바꾸거나 다른 구멍에 꽂으면 이름이 바뀌고, 설정에 박아둔 옛 이름으로는
# 동글을 열지 못해 HOST_FATAL_ERROR로 죽습니다. 그래서 띄울 때마다 USB
# 장치 이름으로 동글을 찾아 그 포트를 넘깁니다.

set -euo pipefail

DONGLE_NAME="${Z2M_DONGLE_NAME:-Sonoff Zigbee 3.0 USB Dongle Plus V2}"
Z2M_DIR="${Z2M_DIR:-$HOME/zigbee2mqtt}"
# z2m이 지원하는 Node 범위가 좁습니다. 기본 node가 벗어나면 serialport가
# 깨져 동글 고장처럼 보입니다. 경로를 고정합니다.
NODE="${Z2M_NODE:-/opt/homebrew/opt/node@24/bin/node}"

port="$(ioreg -r -l -n "$DONGLE_NAME" | sed -n 's/.*"IOCalloutDevice" = "\(.*\)"/\1/p' | head -1)"

if [ -z "$port" ]; then
    echo "$(date '+%F %T') 동글을 찾지 못했습니다: $DONGLE_NAME" >&2
    # 바로 죽으면 LaunchAgent가 15초 뒤 다시 부릅니다. 꽂히면 그때 뜹니다.
    exit 1
fi

echo "$(date '+%F %T') 동글 포트: $port"

cd "$Z2M_DIR"
# 설정 파일의 serial.port를 이 값으로 덮어씁니다.
export ZIGBEE2MQTT_CONFIG_SERIAL_PORT="$port"
exec "$NODE" index.js
