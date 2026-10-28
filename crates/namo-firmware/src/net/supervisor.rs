//! 접속된 뒤의 네트워크 감독. 스펙 `docs/pilot-design.md` §4.7.
//!
//! 공유기가 재시작하면 WiFi가 끊기는데 ESP-IDF는 스스로 다시 붙지 않습니다.
//! 여기서 1초마다 점검해 다시 붙이고, 그래도 MQTT가 오래 안 붙으면
//! 재부팅합니다. 판정은 `namo-core::link`에 있습니다.

use esp_idf_svc::wifi::EspWifi;
use namo_core::link::{LinkWatchdog, Reconnect, Verdict};
use std::time::{Duration, Instant};

use crate::config::{MQTT_DOWN_RESTART_S, NET_CHECK_PERIOD_S};
use crate::net::wifi;
use crate::state::{PumpState, Shared};

/// 돌아오지 않습니다. WiFi 핸들을 쥐고 있으므로, 이 함수가 끝나면 WiFi가
/// 내려갑니다.
pub fn run(mut esp_wifi: EspWifi<'static>, shared: Shared) -> ! {
    let boot = Instant::now();
    let mut reconnect = Reconnect::new();
    let mut watchdog = LinkWatchdog::new(MQTT_DOWN_RESTART_S);
    // 여기까지 왔다면 부팅 때 접속에 성공한 것입니다.
    let mut was_up = true;

    loop {
        std::thread::sleep(Duration::from_secs(NET_CHECK_PERIOD_S));
        let now_s = boot.elapsed().as_secs();

        let up = wifi::link_up(&esp_wifi);
        if up != was_up {
            if up {
                let ip = esp_wifi.sta_netif().get_ip_info().map(|i| i.ip);
                log::info!("WiFi 다시 붙었습니다. IP {ip:?}");
            } else {
                log::warn!("WiFi가 끊겼습니다. 다시 붙을 때까지 재시도합니다.");
            }
            was_up = up;
        }
        if reconnect.should_try(now_s, up) {
            wifi::reconnect(&mut esp_wifi);
        }

        // 락이 깨졌으면 이번 점검은 건너뜁니다. 판단할 근거가 없습니다.
        let Some((mqtt_up, pump_idle)) = shared
            .lock()
            .ok()
            .map(|s| (s.mqtt_connected, s.pump == PumpState::Idle))
        else {
            continue;
        };

        if watchdog.observe(now_s, mqtt_up, pump_idle) == Verdict::Restart {
            log::error!(
                "MQTT가 {}분 넘게 붙지 않아 재부팅합니다.",
                MQTT_DOWN_RESTART_S / 60
            );
            esp_idf_svc::hal::reset::restart();
        }
    }
}
