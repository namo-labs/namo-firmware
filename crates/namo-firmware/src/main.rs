//! 나모 파일럿 펌웨어. 스펙 `docs/pilot-design.md` §4.4.
//!
//! 초기화 순서가 안전규칙의 일부입니다. 펌프 핀을 내리는 것이 가장 먼저이며,
//! 그 뒤에 무엇이 실패하든 펌프는 돌지 않습니다(S1).

use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::gpio::{AnyIOPin, AnyOutputPin};
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use namo_firmware::config::{FLOAT_POLL_MS, PIN_FLOAT, PIN_PUMP};
use namo_firmware::hw::float::FloatSwitch;
use namo_firmware::hw::pump::Pump;
use namo_firmware::net::mqtt::Topics;
use namo_firmware::state::{new_shared, Shared};
use namo_firmware::{ble, clock, net, telemetry, worker};

#[toml_cfg::toml_config]
pub struct Config {
    #[default("")]
    wifi_ssid: &'static str,
    #[default("")]
    wifi_psk: &'static str,
    #[default("")]
    mqtt_url: &'static str,
    #[default("pilot01")]
    device_id: &'static str,
}

fn main() -> anyhow::Result<()> {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    let peripherals = Peripherals::take()?;

    // ── S1 ────────────────────────────────────────────────────────
    // 이 문장이 가장 먼저입니다. WiFi·BLE보다, 설정 검증보다 앞섭니다.
    // 아래 어느 줄에서 에러로 빠져나가도 펌프는 이미 LOW입니다.
    // SAFETY: 이 핀은 여기서만 가져오며 Pump가 소유권을 독점합니다.
    let pump_pin = unsafe { AnyOutputPin::steal(PIN_PUMP) };
    let pump = Pump::new(pump_pin)?;
    log::info!("펌프 GPIO{PIN_PUMP}를 LOW로 확정했습니다");

    if CONFIG.wifi_ssid.is_empty() || CONFIG.mqtt_url.is_empty() {
        anyhow::bail!("cfg.toml이 비어 있습니다. cfg.toml.example을 복사해 채우세요.");
    }

    let shared = new_shared();
    let topics = Arc::new(Topics::new(CONFIG.device_id));

    // ── 플로트 폴링 ───────────────────────────────────────────────
    // 네트워크보다 먼저 띄웁니다. 물통 상태는 급수 판정의 입력이고,
    // 브로커가 없어도 알아야 하는 값입니다.
    // SAFETY: 이 핀도 여기서만 가져옵니다.
    let float_pin = unsafe { AnyIOPin::steal(PIN_FLOAT) };
    let float = FloatSwitch::new(float_pin)?;
    spawn("io_poll", 3000, {
        let shared = shared.clone();
        move || io_poll(float, shared)
    })?;

    // ── 네트워크 ──────────────────────────────────────────────────
    let sys_loop = EspSystemEventLoop::take()?;
    let nvs = EspDefaultNvsPartition::take()?;

    let _wifi = net::wifi::connect(
        peripherals.modem,
        sys_loop,
        nvs,
        CONFIG.wifi_ssid,
        CONFIG.wifi_psk,
    )?;

    // SNTP가 실패해도 계속 갑니다. 시각을 모르면 급수만 거부되고
    // 센서 수집과 텔레메트리는 그대로 의미가 있습니다.
    let sntp = clock::start_sntp()?;
    if clock::wait_synced(&sntp, 15) {
        log::info!("시각 동기화됨");
    } else {
        log::warn!("시각 동기화 실패. 급수는 거부되고 센서 수집만 계속합니다.");
    }

    let (publisher, connection) = net::mqtt::connect(CONFIG.mqtt_url, CONFIG.device_id, &topics)?;

    // ── 태스크 기동 ───────────────────────────────────────────────
    let (tx, rx) = mpsc::channel();

    spawn("mqtt", 6000, {
        let publisher = publisher.clone();
        let topics = topics.clone();
        let shared = shared.clone();
        move || net::mqtt::run_event_loop(connection, publisher, topics, shared, tx)
    })?;

    spawn("pump_worker", 6000, {
        let shared = shared.clone();
        let publisher = publisher.clone();
        let topics = topics.clone();
        move || worker::run(rx, pump, shared, publisher, topics)
    })?;

    spawn("telemetry", 6000, {
        let shared = shared.clone();
        let publisher = publisher.clone();
        let topics = topics.clone();
        move || telemetry::run(shared, publisher, topics)
    })?;

    // BLE 스택은 스택을 많이 씁니다.
    spawn("ble_scan", 8000, {
        let shared = shared.clone();
        move || ble::scan::run(shared)
    })?;

    // 브로커가 없어도 브라우저로 상태를 볼 수 있게 합니다. 읽기 전용이라
    // 여기로는 급수를 시킬 수 없습니다.
    let _http = net::http::start(shared.clone())?;

    log::info!("모든 태스크가 기동됐습니다");

    // main 스레드가 끝나면 _wifi와 sntp가 떨어져 네트워크가 내려갑니다.
    // 여기서 붙잡아 둡니다.
    loop {
        std::thread::sleep(Duration::from_secs(60));
    }
}

/// 플로트 스위치를 주기적으로 읽어 공유 상태에 반영합니다.
fn io_poll(mut float: FloatSwitch<'static>, shared: Shared) {
    let mut last = None;
    loop {
        let reservoir = float.poll();

        if let Ok(mut state) = shared.lock() {
            state.reservoir = reservoir;
        }

        if last != Some(reservoir) {
            log::info!("물통 상태: {reservoir:?}");
            last = Some(reservoir);
        }

        std::thread::sleep(Duration::from_millis(FLOAT_POLL_MS));
    }
}

/// 이름과 스택 크기를 지정해 스레드를 띄웁니다.
///
/// ESP-IDF의 기본 스택은 작아서 BLE나 MQTT 처리 중에 넘칠 수 있습니다.
/// 넘치면 조용히 죽는 것이 아니라 보드가 리셋되므로, 태스크마다 넉넉히 줍니다.
fn spawn<F>(name: &str, stack_size: usize, f: F) -> anyhow::Result<()>
where
    F: FnOnce() + Send + 'static,
{
    std::thread::Builder::new()
        .name(name.to_string())
        .stack_size(stack_size)
        .spawn(f)?;
    Ok(())
}
