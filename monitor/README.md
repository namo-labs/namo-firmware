# namo-monitor

파일럿의 텔레메트리를 모아 웹으로 보여주는 서비스입니다.

ESP32도 상태 페이지를 갖고 있지만 지금 값만 보여주고 집 안에서만 열립니다.
식물을 키울 때 정작 필요한 것은 "지금 몇 퍼센트"보다 **"물을 준 뒤로 어떻게
말라왔나"** 입니다. 그래서 이력을 쌓습니다.

ESP32를 직접 외부에 노출하지 않기 위한 것이기도 합니다. 작은 기기에 공개
트래픽이 꽂히면 부담이고, 재부팅 중에는 아예 응답하지 못합니다.

## 하는 일

- `namo/pilot/<id>/telemetry`를 구독해 센서 이력을 쌓습니다.
- `namo/pilot/<id>/water/result`를 구독해 급수를 기록합니다. 그래프에 세로선으로
  찍히므로, 준 시점과 그 뒤의 곡선을 함께 볼 수 있습니다.
- 이력은 파일에 남아 재시작해도 유지됩니다.

텔레메트리는 통째로 보관하고 이력에 담을 필드만 추립니다. 펌웨어가 필드를
늘려도 이쪽을 고치지 않아도 됩니다.

## 엔드포인트

| 경로 | 내용 |
|---|---|
| `/stats` | 화면 (현재값 · 흙수분 그래프 · 상세) |
| `/api/state` | 마지막 텔레메트리 + 수신 경과 + 카메라 생존 |
| `/api/history?hours=N` | 최근 N시간 표본 |
| `/healthz` | 헬스체크 |

## 설정

| 환경변수 | 기본값 | 설명 |
|---|---|---|
| `MQTT_URL` | `tcp://192.168.5.2:1883` | lima VM에서 맥 호스트가 이 주소입니다 |
| `DEVICE_ID` | `pilot01` | |
| `DATA_DIR` | `/data` | 이력 파일 위치 |
| `ADDR` | `:8080` | |
| `MIN_GAP_S` | `60` | 표본 최소 간격. 텔레메트리는 10초마다 오지만 흙수분은 분 단위로도 거의 변하지 않습니다 |
| `MAX_AGE_S` | `2592000` | 이력 보존 기간 (30일) |
| `CAM_PROBE_URL` | `http://192.168.5.2:8090/stream.m3u8` | 카메라 생존을 확인할 곳 |
| `CAM_STALL_S` | `25` | 플레이리스트가 이만큼 갱신되지 않으면 멈춘 것으로 봅니다 |

## 개발

```bash
cd monitor
go test ./...

MQTT_URL=tcp://localhost:1883 DATA_DIR=/tmp/namo ADDR=:8099 MIN_GAP_S=5 go run .
# http://localhost:8099/stats
```

## 배포

`master`에 `monitor/` 변경이 올라가면 GitHub Actions가
`ghcr.io/namo-labs/namo-monitor:latest`를 만듭니다(amd64·arm64).

k8s 매니페스트는 홈서버 쪽에 있습니다.

```bash
kubectl apply -k ~/homeserver/k8s/base
kubectl rollout restart deploy/namo-monitor -n homeserver   # latest 태그를 다시 받게
```

`kang1027.com/namo/stats`로 열리며 Caddy가 Basic Auth를 겁니다. 자격증명은
`~/homeserver/k8s/secrets/namo-monitor.env`에 있고, k8s Secret
`namo-monitor-auth`로 들어갑니다.

**이미지는 public이어야 합니다.** private이면 k3s가 받지 못해
`ImagePullBackOff`가 납니다.
