//! WiFi 접속.

use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::modem::Modem;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::wifi::{BlockingWifi, ClientConfiguration, Configuration, EspWifi};
use esp_idf_svc::sys::EspError;
use namo_core::link::backoff_s;
use std::time::Duration;

/// 처음 접속될 때까지 재시도합니다. 성공하면 핸들을 돌려줍니다.
///
/// 반환값을 떨어뜨리면 WiFi가 내려갑니다. 호출자가 계속 들고 있어야 합니다.
///
/// **성공할 때까지 포기하지 않습니다.** 한 번 실패하고 끝내면 공유기가 잠깐
/// 재시작하거나 접속이 한 번 튕긴 것만으로 장치가 죽습니다. 실제로 결합
/// 단계에서 간헐적으로 거부당하는 것을 확인했고, 다음 시도에서는 붙었습니다.
///
/// **부팅 때만 도는 루프입니다.** 접속된 뒤 끊기면 ESP-IDF가 스스로 다시
/// 붙지 않으므로, 그 뒤는 [`super::supervisor`]가 맡습니다.
pub fn connect(
    modem: Modem<'static>,
    sys_loop: EspSystemEventLoop,
    nvs: EspDefaultNvsPartition,
    ssid: &str,
    psk: &str,
) -> Result<EspWifi<'static>, EspError> {
    let mut esp_wifi = EspWifi::new(modem, sys_loop.clone(), Some(nvs))?;

    {
        let mut wifi = BlockingWifi::wrap(&mut esp_wifi, sys_loop)?;

        wifi.set_configuration(&Configuration::Client(ClientConfiguration {
            ssid: ssid.try_into().map_err(|_| {
                // heapless 변환 실패는 길이 초과뿐입니다.
                EspError::from_infallible::<{ esp_idf_svc::sys::ESP_ERR_INVALID_ARG }>()
            })?,
            password: psk.try_into().map_err(|_| {
                EspError::from_infallible::<{ esp_idf_svc::sys::ESP_ERR_INVALID_ARG }>()
            })?,
            ..Default::default()
        }))?;

        wifi.start()?;

        for attempt in 1u32.. {
            match wifi.connect().and_then(|()| wifi.wait_netif_up()) {
                Ok(()) => break,
                Err(e) => {
                    // 다음 시도가 깨끗한 상태에서 시작하도록 정리합니다.
                    // 이미 끊겨 있으면 실패하는데, 그건 무시해도 됩니다.
                    let _ = wifi.disconnect();

                    let backoff = backoff_s(attempt);
                    log::warn!("WiFi 접속 실패 ({attempt}회차): {e}. {backoff}초 후 다시 시도합니다.");
                    std::thread::sleep(Duration::from_secs(backoff));
                }
            }
        }
    }

    let ip = esp_wifi.sta_netif().get_ip_info()?.ip;
    log::info!("WiFi 접속됨. IP {ip}");

    Ok(esp_wifi)
}

/// 결합돼 있고 IP까지 받았는지.
///
/// 결합만 되고 IP가 없으면 MQTT는 `Host is unreachable`로 실패합니다.
pub fn link_up(wifi: &EspWifi<'static>) -> bool {
    wifi.is_connected().unwrap_or(false)
        && wifi
            .sta_netif()
            .get_ip_info()
            .map(|info| !info.ip.is_unspecified())
            .unwrap_or(false)
}

/// 끊긴 WiFi를 다시 붙이려 한 번 시도합니다. 기다리지 않습니다.
///
/// 결과는 다음 점검에서 [`link_up`]으로 봅니다. 이미 결합 중이면 `connect`가
/// 에러를 돌려주는데, 진행 중이라는 뜻이라 무시해도 됩니다.
///
/// 결합은 됐는데 IP가 없는 경우에는 결합부터 풀어, 다음 시도가 처음부터
/// 다시 맺고 DHCP를 새로 받게 합니다.
pub fn reconnect(wifi: &mut EspWifi<'static>) {
    if wifi.is_connected().unwrap_or(false) {
        if let Err(e) = wifi.disconnect() {
            log::warn!("WiFi 결합을 풀지 못했습니다: {e}");
        }
        return;
    }
    if let Err(e) = wifi.connect() {
        log::warn!("WiFi 재접속 시도 실패: {e}");
    }
}
