# 나모 자택 파일럿 펌웨어 설계

> 버전: v0.1 · 작성일: 2026-09-01 · 대상: ESP32-S3 + 맥 게이트웨이

## 1. 목적

보유한 하드웨어 전 기능을 실제로 동작시키고, 원격에서 상태를 확인하고 급수 명령을 내릴 수 있는 상태까지 만듭니다.

이번 파일럿은 **장비 검증**이 목적입니다. Namo 앱의 도메인 계약이나 백엔드 API에 맞추지 않습니다. 앱은 별도 담당자가 진행하며, 이 레포는 하드웨어와 펌웨어만 책임집니다.

### 포함

- ESP32-S3 Rust 펌웨어
- HHCC BLE passive 수집 (흙 수분·온도·조도·전도도)
- 플로트 스위치 입력 (물통 유무)
- MOSFET을 통한 펌프 ON/OFF 제어와 정량 급수
- 펌웨어 내부에서 단독 동작하는 로컬 안전규칙
- WiFi + MQTT 텔레메트리 발행 및 급수 명령 구독
- ESP32 자체 HTTP 상태 페이지 (브로커 없이 디버깅)
- 맥 게이트웨이: Mosquitto + Zigbee2MQTT + Aqara 누수센서

### 제외

- Namo 앱 연동, 백엔드 HTTP API, 사용자 인증
- 카메라 캡처와 영상 변환 (후순위)
- 센서 기반 자동급수 정책
- 화분 2개 이상의 다중 제어
- OTA 펌웨어 업데이트

## 2. 시스템 구성

```text
                     ┌──────────────────────────────────────┐
                     │            ESP32-S3                  │
   HHCC ─ BLE adv ──▶│  ble: MiBeacon passive 파서           │
                     │  io:  플로트 스위치 (GPIO in)          │
   펌프 ◀─ AOD4184 ──│  hw:  펌프 드라이버 (GPIO out)         │
                     │  safety: 로컬 안전규칙 (단독 동작)      │
                     │  net: WiFi · MQTT · HTTP 상태 페이지   │
                     └───────────────┬──────────────────────┘
                                     │ WiFi
                                     ▼
   Aqara 누수 x2 ─ Zigbee ─▶ ZBDongle-E ─▶ Zigbee2MQTT ─▶ Mosquitto
                                                              (맥, Docker)
                                                                  ▲
                                                                  │
                                                    MQTT 클라이언트(맥·폰)
```

### 책임 경계

| 계층 | 책임 | 네트워크가 끊기면 |
|---|---|---|
| ESP32-S3 | 센서 수집, 펌프 구동, **안전 판정** | 안전규칙은 그대로 동작. 진행 중 급수는 로컬 타이머로 정상 종료 |
| Mosquitto | 메시지 중계 | 텔레메트리 유실. 급수 명령 전달 불가 |
| Zigbee2MQTT | 누수센서 수집 | 누수 정보가 stale이 되어 ESP32가 급수를 차단 |

**안전 판정을 절대 게이트웨이에 두지 않습니다.** 브로커가 죽거나 WiFi가 끊긴 상태에서도 펌프가 폭주하지 않아야 합니다.

## 3. 하드웨어 설계

### 3.1 핀 배정

ESP32-S3의 스트래핑 핀(GPIO0·3·45·46), 플래시/PSRAM 핀(GPIO26~32, 옥탈 PSRAM이면 33~37), USB 핀(GPIO19·20)을 모두 피한 조합입니다.

| 기능 | GPIO | 방향 | 설정 | 비고 |
|---|---|---|---|---|
| 펌프 제어 (MOSFET 게이트) | 4 | output | 부팅 즉시 LOW | **외부 10kΩ 풀다운 필수** |
| 플로트 스위치 | 5 | input | 내부 풀업 | 반대편은 GND. 100ms 디바운스 |
| 상태 LED | 48 | output | - | DevKitC-1 내장 RGB. 보드에 없으면 생략 |

GPIO 배정을 바꿔야 하면 `pilot-design.md`의 이 표를 먼저 고치고 `config.rs`를 맞춥니다.

#### 플로트 스위치 논리

실물 측정 결과(H7, 2026-09-07) 플로트가 **위로 뜨면 도통, 내려가면 개방**입니다. GPIO5에 내부 풀업을 걸고 반대편을 GND에 물리므로 다음과 같이 읽힙니다.

| 물통 | 플로트 | 접점 | GPIO5 | 판정 |
|---|---|---|---|---|
| 물 있음 | 뜸 | 도통 | **LOW** | `ok` |
| 물 없음 | 내려감 | 개방 | **HIGH** | `empty` |

이 방향이 안전한 쪽입니다. 배선이 빠지거나 끊어지면 접점이 열린 것과 구별되지 않는데, 그 상태가 HIGH = `empty`로 읽혀 급수가 거부됩니다. 반대 방향(뜨면 개방)이었다면 단선이 "물 있음"으로 읽혀 빈 물통에서 펌프가 돌았을 것입니다.

단선과 진짜 빈 물통을 구별하지는 못하므로, 둘 다 `empty`로 취급하고 급수를 막습니다. `unknown`은 디바운스가 아직 안정되지 않은 부팅 직후에만 씁니다.

### 3.2 전원 계통

```text
12V 어댑터 (+) ──┬─────────────────────── 펌프 (+)
                 │                          │
                 │        1N4007            │
                 └──── 캐소드(띠) ◀─ 애노드 ─┘
                                            │
                                     펌프 (−)
                                            │
                                            ▼
                              AOD4184  [D] 단자
                                       [S] 단자 ── 12V 어댑터 (−)
                                       [SIG]    ── ESP32 GPIO4
                                       [GND]    ── ESP32 GND ── 12V 어댑터 (−)
                                                     (공통 접지)
                              GPIO4 ── 10kΩ ── GND

ESP32 전원: USB 별도 급전 (파일럿 단계)
```

설계 근거는 다음과 같습니다.

**게이트 풀다운 10kΩ은 옵션이 아닙니다.** ESP32는 리셋 직후와 부팅 중 GPIO가 입력(하이임피던스) 상태입니다. 풀다운이 없으면 MOSFET 게이트가 뜬 채로 남아 정전기나 누설전류로 펌프가 임의로 돌 수 있습니다. 물통 옆에서 이 사고가 나면 바닥이 젖습니다.

**1N4007 방향은 캐소드(띠 있는 쪽)가 +12V입니다.** 평상시에는 역방향이라 전류가 흐르지 않고, 펌프를 끄는 순간 코일에 생기는 역기전력만 돌려보냅니다. 방향을 반대로 꽂으면 전원을 켜자마자 단락됩니다.

**공통 접지가 없으면 MOSFET이 켜지지 않습니다.** ESP32를 USB로 따로 급전하면 12V 계통과 기준 전위가 달라집니다. GPIO4의 3.3V는 ESP32 GND 기준값이므로, 두 GND를 묶어야 게이트 전압이 성립합니다. 초보자가 가장 많이 놓치는 지점입니다.

**PWM을 쓰지 않습니다.** 1N4007의 역회복이 느려 PWM 스위칭에서 다이오드와 MOSFET이 모두 발열합니다. 급수량은 PWM 듀티가 아니라 **구동 시간(초)** 으로 조절합니다.

### 3.3 급수량 환산

앱이나 사용자는 mL로 말하고, 펌웨어는 초로 환산합니다.

```text
pump_ms = dose_ml / ml_per_second * 1000
```

구현은 `no_std`에서 부동소수 라이브러리 의존을 피하려고 이 계산을 µL/s 정수 산술로 합니다. `cfg.toml`에는 사람이 읽는 `ml_per_second`(예: 6.5)를 두고, 펌웨어가 `ul_per_second`(6500)로 변환해 `namo-core`에 넘깁니다.

`ml_per_second`는 카탈로그 값이 아니라 **실제 설치 높이에서 실측한 값**입니다. 수중 펌프의 유량은 양정(펌프 수면부터 토출구까지의 높이)에 크게 좌우돼 카탈로그의 240L/h는 무양정 기준입니다. 실측은 `bring-up-guide.md` Stage 4에서 합니다.

실측값은 `cfg.toml`에 상수로 넣고, 물통 위치나 튜브 배치를 바꾸면 다시 측정합니다.

## 4. 펌웨어 설계

### 4.1 스택 선택

**`std` Rust + ESP-IDF 기반**을 씁니다.

| 후보 | 판단 |
|---|---|
| `esp-idf-svc` (std) | **채택.** BLE central 스캔과 WiFi 동시 동작이 NimBLE로 검증돼 있고, MQTT·HTTP·SNTP가 모두 포함됨 |
| `esp-hal` (no_std) | 기각. `esp-wifi`의 WiFi+BLE coexistence가 ESP32-S3에서 아직 불안정해, 파일럿이 아니라 크레이트 디버깅이 될 위험이 큼 |
| ESPHome (YAML) | 기각. Rust로 구현한다는 전제를 만족하지 못함 |

no_std는 파일럿이 안정된 뒤 재검토 대상입니다. 지금 선택의 근거는 "가장 Rust다워서"가 아니라 "BLE와 WiFi를 동시에 확실히 켤 수 있어서"입니다.

### 4.2 크레이트

| 크레이트 | 버전 | 용도 |
|---|---|---|
| `esp-idf-svc` | 0.52 | WiFi, MQTT, HTTP 서버, SNTP, NVS |
| `esp-idf-hal` | (svc가 재수출) | GPIO, 타이머 |
| `esp32-nimble` | 0.12 | BLE 스캔 |
| `serde` / `serde_json` | 1 | MQTT 페이로드 |
| `anyhow` | 1 | 에러 |
| `log` | 0.4 | 로깅 |

툴체인은 `espup`으로 설치하는 Xtensa 전용 Rust 툴체인이 필요합니다. 플래싱은 `cargo-espflash`를 씁니다.

### 4.3 크레이트 구조

```text
Cargo.toml                  워크스페이스
crates/
  namo-core/                no_std · 하드웨어 의존 없음 · 맥에서 cargo test 가능
    src/
      mibeacon.rs           MiBeacon advertising 파서
      telemetry.rs          센서 상태 집계와 신선도 판정
      safety.rs             급수 허용 여부 판정 (순수 함수)
      dose.rs               mL ↔ 구동시간 환산
      command.rs            명령 타입 · TTL · 멱등성 키
  namo-firmware/            ESP32-S3 바이너리 · std
    src/
      main.rs               초기화 순서와 태스크 기동
      config.rs             핀·WiFi·MQTT·한도 상수
      hw/pump.rs            펌프 드라이버 (하드리밋 내장)
      hw/float.rs           플로트 스위치 디바운스
      ble/scan.rs           NimBLE 스캔 → namo-core 파서 호출
      net/wifi.rs
      net/mqtt.rs           토픽 구독·발행
      net/http.rs           상태 페이지
      state.rs              공유 상태 (Mutex)
```

**핵심 판정 로직을 `namo-core`에 몰아넣는 이유**는 테스트 때문입니다. ESP32 타겟에서는 `cargo test`를 돌리기 번거롭습니다. 파서와 안전규칙을 하드웨어 의존 없는 순수 함수로 분리하면 맥에서 그냥 `cargo test -p namo-core`로 검증할 수 있습니다. 물을 실제로 뿌리기 전에 안전규칙을 테스트로 먼저 굳히는 것이 이 구조의 목적입니다.

### 4.4 태스크 구조

`esp-idf-svc`는 std 환경이라 OS 스레드를 씁니다.

| 태스크 | 주기 | 책임 |
|---|---|---|
| `main` | - | GPIO 안전 초기화 → WiFi → SNTP → MQTT → 태스크 기동 |
| `ble_scan` | 상시 | BLE 광고 수신 → MiBeacon 파싱 → 공유 상태 갱신 |
| `io_poll` | 100ms | 플로트 스위치 디바운스 읽기 |
| `pump_worker` | 이벤트 | 명령 수신 → 안전 판정 → 구동 → **100ms마다 중단조건 재검사** → 결과 발행 |
| `telemetry_pub` | 10초 | 현재 상태를 MQTT로 발행 |
| `http_server` | 요청 시 | 상태 JSON과 간이 HTML 페이지 |

공유 상태는 `Arc<Mutex<SharedState>>` 하나로 통일합니다. 락을 잡은 채로 네트워크 I/O를 하지 않습니다.

### 4.5 로컬 안전규칙

**모두 ESP32 안에서 판정합니다.** 게이트웨이나 네트워크에 의존하지 않습니다.

| # | 규칙 | 동작 |
|---|---|---|
| S1 | 부팅 즉시 펌프 핀을 출력 LOW로 확정 | `main` 최초 문장. WiFi보다 먼저 |
| S2 | 하드웨어 풀다운 10kΩ | 부팅 전 구간을 소프트웨어 없이 커버 |
| S3a | 펌프 최대 연속 구동시간 20초 | 어떤 명령도 이 값을 넘길 수 없음. 드라이버 내부에서 강제 |
| S3b | 1회 명령 최대 급수량 300mL | 초과 요청은 `rejected(dose_too_large)`. 클램프하지 않고 거부 |
| S4 | 물통이 비면 급수 거부 | 구동 중 감지되면 **즉시 중단**하고 `aborted_reservoir_empty` 발행 |
| S5 | 누수 감지 시 즉시 중단 후 잠금 | 수동 해제(`unlock` 명령) 전까지 모든 급수 거부 |
| S6 | 급수 완료 후 쿨다운 30분 | 연속 급수로 인한 과습 방지 |
| S7 | 일일 총 급수량 한도 500mL | 자정(로컬 시각) 기준 리셋 |
| S8 | 명령 TTL 초과 시 무실행 거부 | 브로커 재연결로 뒤늦게 도착한 명령이 실행되는 것을 차단 |
| S9 | 같은 명령 ID 재수신 시 재실행 안 함 | 최근 명령 ID 16개를 링버퍼로 보관 |
| S10 | 누수 정보가 5분 이상 stale이면 급수 거부 | fail-closed. "모르면 안 준다" |
| S11 | 진행 중 급수는 WiFi가 끊겨도 로컬 타이머로 정상 종료 | 네트워크 상태와 무관하게 펌프는 반드시 꺼짐 |

S3a와 S11이 최후의 방어선입니다. 나머지 규칙이 전부 실패해도 펌프는 20초 안에 반드시 꺼집니다.

S3a와 S3b는 중복이 아니라 서로 다른 것을 막습니다. S3b는 잘못 계산된 큰 요청을 입구에서 거르고, S3a는 시간 계산 자체가 틀렸을 때를 막습니다. 실효 상한은 둘 중 먼저 걸리는 쪽입니다. **`ml_per_second`를 실측한 뒤(Stage 4) 20초가 몇 mL에 해당하는지 계산해, 두 값의 관계를 확인하고 필요하면 조정합니다.** 물통이 2L이므로 1회 급수가 물통을 절반 이상 비우면 안 됩니다.

S10의 한계를 명시합니다. 브로커가 죽으면 누수 정보가 stale이 되어 급수가 아예 불가능해집니다. 이는 의도된 동작입니다. 물을 못 주는 것보다 누수를 모른 채 물을 주는 것이 훨씬 나쁩니다.

### 4.6 급수 명령 수명주기

```text
requested ──▶ [안전 판정] ──▶ rejected (사유 포함)
                   │
                   ▼
                running ──▶ completed
                   │
                   ├──▶ aborted   (구동 중 안전조건 위반)
                   └──▶ failed    (하드웨어 오류)
```

- 종료 상태는 `completed` · `rejected` · `aborted` · `failed` 넷입니다.
- **TTL이 만료된 명령은 상태를 만들지 않고 조용히 버립니다.** 실행하지 않았음이 보장돼야 하기 때문입니다.
- 진행 중 명령이 있으면 새 명령은 `rejected(already_running)`입니다.
- 실제 급수량은 유량계 실측이 아니라 `ml_per_second` 기반 추정치입니다. 페이로드 필드명을 `estimated_ml`로 두어 추정임을 드러냅니다.

## 5. MQTT 계약

브로커는 맥의 Mosquitto입니다. `DEVICE_ID`는 ESP32의 MAC 기반으로 생성합니다.

### 5.1 토픽

| 토픽 | 방향 | retain | 설명 |
|---|---|---|---|
| `namo/pilot/<id>/status` | 발행 | ✅ | `online` / `offline`. LWT로 `offline` 등록 |
| `namo/pilot/<id>/telemetry` | 발행 | ❌ | 센서·물통·펌프 상태 (10초) |
| `namo/pilot/<id>/water/cmd` | 구독 | ❌ | 급수 명령. **retain 금지** |
| `namo/pilot/<id>/water/result` | 발행 | ❌ | 명령 결과 |
| `namo/pilot/<id>/unlock` | 구독 | ❌ | 누수 잠금(S5) 수동 해제. 페이로드 `{"id":"..."}` |
| `zigbee2mqtt/leak_tank` | 구독 | - | 물통 주변 누수센서 |
| `zigbee2mqtt/leak_pot` | 구독 | - | 화분 주변 누수센서 |

`water/cmd`에 retain을 걸면 ESP32가 재부팅할 때마다 마지막 명령이 재전달돼 물을 다시 줍니다. S8(TTL)과 S9(멱등성)로 이중 방어하지만, 애초에 retain을 걸지 않는 것이 1차 방어입니다.

### 5.2 텔레메트리 페이로드

```json
{
  "ts": 1772500000,
  "soil_moisture_pct": 34,
  "soil_temperature_c": 22.4,
  "illuminance_lux": 1820,
  "soil_conductivity_us_cm": 340,
  "sensor_seen_ago_s": 47,
  "reservoir": "ok",
  "leak": "none",
  "leak_seen_ago_s": 12,
  "pump": "idle",
  "today_estimated_ml": 150,
  "cooldown_until": 1772501800,
  "locked": false
}
```

- 아직 한 번도 수집하지 못한 센서값은 `null`입니다. 임의의 기본값으로 채우지 않습니다.
- `reservoir`는 `ok` / `empty` / `unknown`, `leak`은 `none` / `detected` / `unknown`입니다.
- `pump`는 `idle` / `running` / `locked`입니다.

### 5.3 급수 명령 페이로드

```json
{
  "id": "01JBQ8Z4K2N7XW",
  "issued_at": 1772500000,
  "ttl_s": 15,
  "dose_ml": 100
}
```

- `id`는 요청 1건당 하나입니다. 재전송할 때는 같은 ID를 씁니다 (S9).
- `ttl_s`는 `issued_at` 기준입니다. ESP32는 SNTP로 시각을 맞춘 뒤에만 TTL을 신뢰합니다. **시각 동기 실패 상태에서는 모든 급수를 거부합니다.**

### 5.4 결과 페이로드

```json
{
  "id": "01JBQ8Z4K2N7XW",
  "status": "completed",
  "reason": null,
  "estimated_ml": 100,
  "pump_ms": 4200,
  "started_at": 1772500001,
  "finished_at": 1772500005
}
```

`reason`은 종료 상태가 `completed`가 아닐 때 항상 채웁니다. 값은 `reservoir_empty` · `reservoir_unknown` · `leak_detected` · `leak_stale` · `cooldown` · `daily_limit` · `already_running` · `clock_unsynced` · `dose_too_large` · `locked` · `hardware_error` 중 하나입니다.

`ttl_expired`는 목록에 없습니다. TTL이 만료된 명령은 **결과를 발행하지 않고 조용히 버리기** 때문입니다(§4.6). 뒤늦게 도착한 명령에 결과를 돌려주면, 그 결과를 보고 재시도하는 흐름이 생겨 이중 급수 위험이 됩니다.

## 6. 게이트웨이 (맥)

게이트웨이는 맥에서 두 프로세스로 돌립니다. **실행 방식이 서로 다릅니다.**

| 서비스 | 실행 방식 | 포트 | 비고 |
|---|---|---|---|
| Mosquitto | Docker (`eclipse-mosquitto`) | 1883 | 파일럿은 익명 접속 허용, LAN 한정 |
| Zigbee2MQTT | **맥에 네이티브 설치** | 8080 | Docker 아님 |

Zigbee2MQTT만 네이티브로 도는 이유는 macOS의 Docker Desktop이 USB 장치를 컨테이너에 넘기지 못하기 때문입니다. 리눅스에서 쓰는 `--device /dev/tty...` 패스스루가 맥에서는 동작하지 않습니다. 설치 절차는 `bring-up-guide.md` Stage 7에 있습니다.

ZBDongle-E는 EmberZNet 기반이므로 어댑터를 `ember`로 설정합니다. TI 기반인 ZBDongle-P와 설정이 다릅니다.

설정 파일은 레포의 `gateway/`에 둡니다.

```text
gateway/
  mosquitto.conf                Mosquitto 설정
  zigbee2mqtt/configuration.yaml Z2M 설정 (시리얼 포트는 각자 환경에 맞게 수정)
  README.md                     실행 명령
```

## 7. 완료 기준

이번 파일럿은 다음이 모두 되면 완료입니다.

1. 맥에서 `cargo test -p namo-core`가 통과합니다 (MiBeacon 파서, 안전규칙, 급수량 환산).
2. HHCC의 네 측정값이 MQTT 텔레메트리에 실제 값으로 올라옵니다.
3. 물통을 비우면 `reservoir`가 5초 안에 `empty`로 바뀝니다.
4. Aqara 누수센서에 물을 묻히면 `leak`이 `detected`로 바뀌고, **진행 중이던 급수가 즉시 중단됩니다.**
5. 폰이나 맥의 MQTT 클라이언트에서 명령을 보내면 실제로 물이 나오고, 계량한 양이 `estimated_ml`의 ±20% 안에 들어옵니다.
6. 급수 직후 재요청이 `cooldown`으로 거부됩니다.
7. WiFi를 끈 상태로 급수를 시작해도 펌프가 정확한 시간에 꺼집니다.
8. ESP32를 리셋해도 펌프가 한 번도 돌지 않습니다.
9. 바질 모종이 2주간 이 시스템으로 살아 있습니다.

## 8. 리스크와 미확정

| 리스크 | 영향 | 대응 |
|---|---|---|
| AOD4184가 3.3V로 완전히 안 켜짐 | 펌프 약하게 돌고 MOSFET 발열 | Stage 2에서 무부하 측정. 실패 시 로직레벨 MOSFET 모듈 추가 구매 또는 3.3V→5V 레벨시프터 |
| HHCC 펌웨어가 advertising에 값을 안 실음 | passive 수집 불가 | GATT 연결 방식으로 전환 (구현량 증가) |
| macOS Docker의 USB 패스스루 제약 | Zigbee2MQTT 실행 불가 | Z2M을 Docker 없이 네이티브 실행. Stage 7 참고 |
| ESP32-S3 옥탈 PSRAM 모델이면 GPIO 충돌 | 부팅 실패 | H2 확인 후 핀 재배정 |
| WiFi + BLE 동시 사용 시 메모리 부족 | 크래시 | PSRAM 활성화, BLE 스캔 버퍼 축소 |
| 튜브가 사이펀 현상으로 물을 계속 빨아냄 | 펌프가 꺼져도 급수 지속 | 토출구를 물통 수면보다 높게 배치. Stage 3에서 확인 |

미확정 항목의 단일 원천은 `hardware-pilot-status.md` §5입니다.
