//! bring-up 보조 도구. 보드가 실제로 보는 WiFi 액세스포인트를 나열합니다.
//!
//! 접속이 타임아웃으로 실패할 때, 원인이 "AP를 못 봄"인지 "인증 실패"인지
//! 구분하기 위해 씁니다. 맥이 보는 목록과 보드가 보는 목록은 다를 수 있습니다.
//! 보드는 2.4GHz만 받으므로 5GHz 전용 AP는 여기 나오지 않습니다.

use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::wifi::{BlockingWifi, ClientConfiguration, Configuration, EspWifi};

fn main() {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    let sys_loop = EspSystemEventLoop::take().expect("이벤트 루프 실패");
    let nvs = EspDefaultNvsPartition::take().expect("NVS 실패");
    let peripherals = Peripherals::take().expect("주변장치 실패");

    let mut esp_wifi =
        EspWifi::new(peripherals.modem, sys_loop.clone(), Some(nvs)).expect("WiFi 생성 실패");
    let mut wifi = BlockingWifi::wrap(&mut esp_wifi, sys_loop).expect("WiFi 래핑 실패");

    wifi.set_configuration(&Configuration::Client(ClientConfiguration::default()))
        .expect("WiFi 설정 실패");
    wifi.start().expect("WiFi 시작 실패");

    log::info!("=== WiFi 스캔 ===");
    log::info!("보드가 받을 수 있는 것만 나옵니다. 5GHz 전용 AP는 나오지 않습니다.");

    // 한 번의 결과가 0개라고 바로 결론짓지 않습니다. RF 초기화 직후나 채널
    // 타이밍이 안 맞아 비는 경우가 있어 여러 번 돌려봅니다.
    for round in 1..=3 {
        match wifi.scan() {
            Ok(aps) => {
                log::info!("[{round}회차] {}개 발견", aps.len());
                for ap in aps.iter() {
                    log::info!(
                        "  {:>4}dBm  ch{:<3} {:?}  \"{}\"",
                        ap.signal_strength,
                        ap.channel,
                        ap.auth_method,
                        ap.ssid
                    );
                }
            }
            Err(e) => log::error!("[{round}회차] 스캔 실패: {e}"),
        }
        std::thread::sleep(std::time::Duration::from_secs(2));
    }

    log::info!("=== 스캔 끝 ===");
}
