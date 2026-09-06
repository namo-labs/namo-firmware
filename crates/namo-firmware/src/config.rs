//! 핀 배정, 한도, 보정값. 스펙 `docs/pilot-design.md` §3.1·§4.5.
//!
//! 값을 바꿀 때는 설계 문서를 먼저 고칩니다.

use namo_core::dose::Calibration;
use namo_core::safety::Limits;

/// 펌프 MOSFET 게이트. 보드 라벨 `D3`.
///
/// 외부 10kΩ 풀다운이 반드시 있어야 합니다(S2). 부팅 전 구간과 리셋 순간은
/// 소프트웨어가 손댈 수 없어 하드웨어로만 커버됩니다.
pub const PIN_PUMP: u8 = 4;

/// 플로트 스위치. 보드 라벨 `D4`. 내부 풀업을 쓰고 반대편은 GND입니다.
///
/// 실측 결과 플로트가 뜨면 도통이라 LOW가 "물 있음"입니다.
pub const PIN_FLOAT: u8 = 5;

/// 펌프 최대 연속 구동시간 (S3a).
///
/// 드라이버가 강제하므로 상위 로직이 이 값을 넘길 수 없습니다. 다른 모든
/// 규칙이 실패해도 펌프는 이 시간 안에 꺼집니다.
pub const MAX_PUMP_MS: u32 = 20_000;

/// 급수 중 중단조건을 다시 보는 주기 (S4·S5).
pub const ABORT_CHECK_MS: u32 = 100;

/// 플로트 스위치 폴링 주기. 디바운서 기본 5회와 곱해 100ms가 됩니다.
pub const FLOAT_POLL_MS: u64 = 20;

/// 텔레메트리 발행 주기.
pub const TELEMETRY_PERIOD_S: u64 = 10;

/// 명령의 `issued_at`이 현재보다 미래일 때 허용할 오차.
///
/// SNTP 동기화 직후나 게이트웨이 시계가 조금 빠른 경우를 흡수합니다. 이보다
/// 크게 미래면 시계를 믿을 수 없다고 보고 거부합니다(fail-closed).
pub const MAX_FUTURE_SKEW_S: u64 = 5;

/// 유량 보정값.
///
/// **Stage 4에서 계량컵으로 실측한 뒤 반드시 교체해야 하는 임시값입니다.**
/// 카탈로그의 240L/h는 무양정 기준이라 실제 배관에서는 한참 낮습니다.
///
/// 이 값이 틀려도 급수량이 어긋날 뿐 위험하지는 않습니다. 시간 계산이
/// 아무리 빗나가도 [`MAX_PUMP_MS`]가 상한을 강제하기 때문입니다(S3a).
pub const CALIBRATION: Calibration = Calibration::from_ml_per_second_x100(650);

/// 안전 한도. `namo-core`의 기본값을 그대로 씁니다.
pub const LIMITS: Limits = Limits::DEFAULT;

/// 누수 센서 토픽. Zigbee2MQTT가 이 이름으로 발행합니다.
pub const TOPIC_LEAK_TANK: &str = "zigbee2mqtt/leak_tank";
/// 화분 주변 누수 센서 토픽.
pub const TOPIC_LEAK_POT: &str = "zigbee2mqtt/leak_pot";
