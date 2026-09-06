//! bring-up 보조 도구. WiFi 접속과 MQTT 브로커 왕복을 확인합니다
//! (`docs/bring-up-guide.md` Stage 6).
//!
//! 실패 지점을 구분할 수 있게 단계마다 로그를 남깁니다. WiFi가 안 붙는
//! 것과 브로커에 못 붙는 것은 원인도 해결책도 다릅니다.
//!
//! 설정은 `cfg.toml`에서 읽습니다. 이 파일은 커밋하지 않습니다.

use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::mqtt::client::{
    EspMqttClient, LwtConfiguration, MqttClientConfiguration, QoS,
};
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::wifi::{BlockingWifi, ClientConfiguration, Configuration, EspWifi};
use std::thread::sleep;
use std::time::Duration;

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

fn main() {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    log::info!("=== 네트워크 점검 ===");

    if CONFIG.wifi_ssid.is_empty() || CONFIG.mqtt_url.is_empty() {
        log::error!("cfg.toml이 비어 있습니다. cfg.toml.example을 복사해 채우세요.");
        return;
    }

    let base = format!("namo/pilot/{}", CONFIG.device_id);
    let topic_status = format!("{base}/status");
    let topic_telemetry = format!("{base}/telemetry");
    let topic_echo = format!("{base}/echo");

    let sys_loop = EspSystemEventLoop::take().expect("이벤트 루프 실패");
    let nvs = EspDefaultNvsPartition::take().expect("NVS 실패");
    let peripherals = Peripherals::take().expect("주변장치 실패");

    // ── 1단계: WiFi ────────────────────────────────────────────────
    log::info!("[1/3] WiFi 접속 시도: {}", CONFIG.wifi_ssid);

    let mut esp_wifi =
        EspWifi::new(peripherals.modem, sys_loop.clone(), Some(nvs)).expect("WiFi 생성 실패");
    let mut wifi = BlockingWifi::wrap(&mut esp_wifi, sys_loop).expect("WiFi 래핑 실패");

    let client_config = ClientConfiguration {
        ssid: CONFIG.wifi_ssid.try_into().expect("SSID가 너무 깁니다"),
        password: CONFIG.wifi_psk.try_into().expect("비밀번호가 너무 깁니다"),
        ..Default::default()
    };

    if let Err(e) = wifi.set_configuration(&Configuration::Client(client_config)) {
        log::error!("WiFi 설정 실패: {e}");
        return;
    }
    if let Err(e) = wifi.start() {
        log::error!("WiFi 시작 실패: {e}");
        return;
    }
    if let Err(e) = wifi.connect() {
        log::error!("WiFi 접속 실패: {e}");
        log::error!("SSID·비밀번호를 확인하세요. 이 보드는 2.4GHz만 됩니다.");
        return;
    }
    if let Err(e) = wifi.wait_netif_up() {
        log::error!("IP 할당 실패: {e}");
        return;
    }

    let ip_info = esp_wifi.sta_netif().get_ip_info().expect("IP 조회 실패");
    log::info!("[1/3] WiFi 접속됨. IP {}", ip_info.ip);

    // ── 2단계: MQTT ────────────────────────────────────────────────
    log::info!("[2/3] MQTT 접속 시도: {}", CONFIG.mqtt_url);

    // 보드 전원이 끊기면 브로커가 대신 offline을 발행합니다. 게이트웨이가
    // 장치 생존을 알 수 있는 유일한 수단입니다.
    let lwt = LwtConfiguration {
        topic: &topic_status,
        payload: b"offline",
        qos: QoS::AtLeastOnce,
        retain: true,
    };

    let (mut client, mut connection) = match EspMqttClient::new(
        CONFIG.mqtt_url,
        &MqttClientConfiguration {
            client_id: Some(CONFIG.device_id),
            lwt: Some(lwt),
            ..Default::default()
        },
    ) {
        Ok(pair) => pair,
        Err(e) => {
            log::error!("MQTT 클라이언트 생성 실패: {e}");
            return;
        }
    };

    std::thread::scope(|scope| {
        // 이벤트를 계속 퍼내지 않으면 publish/subscribe가 동작하지 않습니다.
        std::thread::Builder::new()
            .stack_size(6000)
            .spawn_scoped(scope, || {
                while let Ok(event) = connection.next() {
                    log::info!("[MQTT] {}", event.payload());
                }
                log::warn!("[MQTT] 연결이 닫혔습니다");
            })
            .expect("이벤트 스레드 생성 실패");

        sleep(Duration::from_millis(500));
        log::info!("[2/3] MQTT 접속됨");

        // ── 3단계: 왕복 ────────────────────────────────────────────
        if let Err(e) = client.subscribe(&topic_echo, QoS::AtMostOnce) {
            log::error!("구독 실패: {e}");
        } else {
            log::info!("[3/3] 구독: {topic_echo}");
        }

        if let Err(e) = client.enqueue(&topic_status, QoS::AtLeastOnce, true, b"online") {
            log::error!("status 발행 실패: {e}");
        }

        log::info!("맥에서 이걸 실행하면 텔레메트리가 보입니다:");
        log::info!("  mosquitto_sub -h <브로커IP> -t '{base}/#' -v");

        let mut seq: u32 = 0;
        loop {
            seq += 1;
            let uptime = unsafe { esp_idf_svc::sys::esp_timer_get_time() } / 1_000_000;
            let free_heap = unsafe { esp_idf_svc::sys::esp_get_free_heap_size() };
            let payload = format!(
                r#"{{"seq":{seq},"uptime_s":{uptime},"free_heap":{free_heap},"rssi":null}}"#
            );

            match client.enqueue(&topic_telemetry, QoS::AtMostOnce, false, payload.as_bytes()) {
                Ok(_) => log::info!("발행 #{seq} → {topic_telemetry}"),
                Err(e) => log::error!("발행 실패: {e}"),
            }

            sleep(Duration::from_secs(5));
        }
    });
}
