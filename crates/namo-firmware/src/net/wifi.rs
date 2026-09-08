//! WiFi 접속.

use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::modem::Modem;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::wifi::{BlockingWifi, ClientConfiguration, Configuration, EspWifi};
use esp_idf_svc::sys::EspError;
use std::time::Duration;

/// 재시도 간격. 마지막 값에 도달하면 그 간격을 유지합니다.
const RETRY_BACKOFF_S: [u64; 5] = [2, 5, 10, 20, 30];

/// 접속될 때까지 재시도합니다. 성공하면 핸들을 돌려줍니다.
///
/// 반환값을 떨어뜨리면 WiFi가 내려갑니다. 호출자가 계속 들고 있어야 합니다.
///
/// **성공할 때까지 포기하지 않습니다.** 한 번 실패하고 끝내면 공유기가 잠깐
/// 재시작하거나 접속이 한 번 튕긴 것만으로 장치가 죽습니다. 실제로 결합
/// 단계에서 간헐적으로 거부당하는 것을 확인했고, 다음 시도에서는 붙었습니다.
///
/// 재시도 간격을 점점 늘립니다. 짧은 간격으로 계속 두드리면 공유기가
/// 방어적으로 차단해 오히려 더 늦게 붙습니다.
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

                    let backoff = RETRY_BACKOFF_S[(attempt as usize - 1).min(RETRY_BACKOFF_S.len() - 1)];
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
