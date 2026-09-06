//! 나모 파일럿 펌웨어.
//!
//! 판정 로직은 `namo-core`에 있고 여기에는 하드웨어와 네트워크만 있습니다.
//! 물을 뿌리기 전에 맥에서 검증할 수 있어야 하기 때문입니다.

pub mod ble;
pub mod clock;
pub mod config;
pub mod hw;
pub mod net;
pub mod state;
pub mod telemetry;
pub mod worker;
