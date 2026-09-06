//! WiFi 접속.

use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::modem::Modem;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::wifi::{BlockingWifi, ClientConfiguration, Configuration, EspWifi};
use esp_idf_svc::sys::EspError;

/// 접속이 끝날 때까지 블로킹합니다. 성공하면 핸들을 돌려줍니다.
///
/// 반환값을 떨어뜨리면 WiFi가 내려갑니다. 호출자가 계속 들고 있어야 합니다.
pub fn connect(
    modem: Modem<'static>,
    sys_loop: EspSystemEventLoop,
    nvs: EspDefaultNvsPartition,
    ssid: &str,
    psk: &str,
) -> Result<EspWifi<'static>, EspError> {
    let mut esp_wifi = EspWifi::new(modem, sys_loop.clone(), Some(nvs))?;
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
    wifi.connect()?;
    wifi.wait_netif_up()?;

    let ip = esp_wifi.sta_netif().get_ip_info()?.ip;
    log::info!("WiFi 접속됨. IP {ip}");

    Ok(esp_wifi)
}
