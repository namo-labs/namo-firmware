# 나모 펌웨어 (namo-firmware)

나모 자택 파일럿의 하드웨어와 펌웨어를 담당하는 레포입니다. ESP32-S3에서 Rust로 동작하며, BLE 센서 수집·펌프 제어·로컬 안전규칙·MQTT 발행을 책임집니다.

## 세션 시작 시

`docs/pilot-design.md`(설계 스펙)와 `docs/hardware-pilot-status.md`(하드웨어 사실)를 먼저 읽습니다. 조립 진행 상황은 `docs/bring-up-guide.md`의 Stage 기준으로 파악합니다.

## 현재 단계

파일럿 하드웨어 부품이 전량 도착했고, 설계 문서를 확정한 단계입니다. 펌웨어 구현은 아직 시작하지 않았습니다.

## 작업 경계

이 레포는 **하드웨어·펌웨어와 그 위의 관측·제어 계층**을 다룹니다.

- Namo 앱(Lynx)은 별도 담당자의 `namo` 레포에서 진행합니다.
- `monitor/`는 파일럿을 관측하고 원격 제어하는 서비스입니다. 앱이 나중에 그대로 가져다 쓸 수 있도록, 기능을 HTTP API로 노출해 둡니다.
- 앱의 도메인 타입(`PlantDetailState` 등)에 펌웨어를 맞추지 않습니다. MQTT 계약은 장비의 것이고, 앱에 맞출 일이 있으면 `monitor/`가 흡수합니다. 펌웨어를 앱 사정으로 고치지 않기 위한 경계입니다.

### 진행 방향

```text
1. 씨앗 심기                 ✅ 2026-09-15
2. 원격 급수 (웹 버튼 + API)  ✅
3. USB 웹캠 스트리밍          ✅
4. 카메라 뷰                  ✅
5. 텔레그램 알림              ✅
6. 앱이 쓸 토큰 인증          ← 지금
```

운영 현황과 남은 일은 `docs/operations.md`에 있습니다.

`monitor/`는 파일럿 관측 도구로 시작했지만 앱 백엔드의 원형이 됩니다. 새 기능은 화면보다 **API를 먼저** 만들고 화면이 그것을 쓰게 합니다.

## 문서 역할

| 문서 | 역할 |
|---|---|
| `docs/pilot-design.md` | 설계 결정의 단일 원천. 핀맵·안전규칙·MQTT 계약·펌웨어 구조 |
| `docs/hardware-pilot-status.md` | 하드웨어 **사실**만. 보유 부품, 미확인 항목, 실측값 |
| `docs/operations.md` | 운영 현황. 무엇이 돌고 있는지, 죽으면 어떻게 되살리는지, 남은 일 |
| `docs/bring-up-guide.md` | 조립·검증 절차. Stage 0~9 |
| `docs/shopping-list.md` | 추가 구비 목록 |

설계를 바꿀 때는 `pilot-design.md`를 먼저 갱신한 뒤 코드를 고칩니다. 실물 측정 결과가 설계와 다르면 `hardware-pilot-status.md`를 먼저 갱신합니다.

## 안전 규칙 (코드 작성 시)

- 펌프 GPIO를 출력 LOW로 확정하는 것이 `main`의 **최초 문장**입니다. WiFi·BLE 초기화보다 먼저입니다.
- 펌프 최대 연속 구동시간 하드리밋은 드라이버 내부에 두고, 상위 로직이 우회할 수 없게 합니다.
- 안전 판정은 전부 ESP32 안에서 합니다. 게이트웨이나 네트워크 상태에 의존하지 않습니다.
- 센서 상태를 모르면(`unknown`) 급수를 거부합니다 (fail-closed).
- 급수 관련 로직을 바꿀 때는 `docs/bring-up-guide.md` Stage 8의 T1~T13을 다시 돌립니다.

## 크레이트 구조

```text
crates/namo-core/       no_std · 하드웨어 의존 없음 · 맥에서 cargo test 가능
crates/namo-firmware/   ESP32-S3 바이너리 · std · esp-idf-svc
monitor/                Go · 텔레메트리를 모아 웹으로 보여주는 서비스
```

MiBeacon 파서·안전규칙·급수량 환산 같은 판정 로직은 반드시 `namo-core`에 둡니다. ESP32 타겟에서는 테스트를 돌리기 번거로우므로, 물을 뿌리기 전에 맥에서 검증할 수 있어야 합니다.

## 빌드

```bash
. $HOME/export-esp.sh              # Xtensa 툴체인 (매 셸마다)
cargo test -p namo-core            # 순수 로직 테스트 (호스트)
cargo espflash flash --monitor -p crates/namo-firmware
```

## 규칙

- 사용자 노출 문구와 문서는 해요체 또는 습니다체로 작성합니다.
- 커밋 메시지는 한국어, scope 없는 conventional commits(`feat:`, `fix:`, `docs:` 등)를 사용합니다.
- WiFi 비밀번호와 MQTT 자격증명은 `cfg.toml`에 두고 커밋하지 않습니다. `cfg.toml.example`만 커밋합니다.
