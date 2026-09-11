//! MQTT 구독과 발행. 스펙 `docs/pilot-design.md` §5.

use esp_idf_svc::mqtt::client::{
    EspMqttClient, EspMqttConnection, EventPayload, LwtConfiguration, MqttClientConfiguration, QoS,
};
use esp_idf_svc::sys::EspError;
use namo_core::command::{CommandId, WaterCommand};
use namo_core::safety::RejectReason;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

use crate::clock;
use crate::config::{
    TOPIC_LEAK_POT, TOPIC_LEAK_POT_AVAIL, TOPIC_LEAK_TANK, TOPIC_LEAK_TANK_AVAIL,
};
use crate::state::Shared;

/// 브로커에 보낼 수 있는 클라이언트. 여러 태스크가 나눠 씁니다.
#[derive(Clone)]
pub struct Publisher {
    client: Arc<Mutex<EspMqttClient<'static>>>,
}

impl Publisher {
    /// 발행합니다. 실패해도 패닉하지 않고 로그만 남깁니다.
    ///
    /// 급수 결과를 못 보내는 것보다 펌프가 안 꺼지는 것이 훨씬 나쁩니다.
    /// 네트워크 실패가 제어 흐름을 끊지 않게 합니다.
    pub fn publish(&self, topic: &str, payload: &[u8], retain: bool) {
        let Ok(mut client) = self.client.lock() else {
            log::error!("MQTT 클라이언트 락이 깨졌습니다");
            return;
        };
        if let Err(e) = client.enqueue(topic, QoS::AtLeastOnce, retain, payload) {
            log::error!("발행 실패 ({topic}): {e}");
        }
    }

    /// 구독합니다.
    ///
    /// **MQTT 이벤트 루프 스레드에서 부르면 안 됩니다.** SUBACK을 기다리는데
    /// 그 응답은 이벤트 루프가 받아야 하므로, 자기가 기다리는 것을 자기가
    /// 처리하지 못해 멈춥니다.
    fn subscribe(&self, topic: &str) {
        let Ok(mut client) = self.client.lock() else {
            return;
        };
        match client.subscribe(topic, QoS::AtLeastOnce) {
            Ok(_) => log::info!("구독: {topic}"),
            Err(e) => log::error!("구독 실패 ({topic}): {e}"),
        }
    }
}

/// 토픽 이름 모음. 장치 ID가 들어가므로 실행 중에 만듭니다.
pub struct Topics {
    pub status: String,
    pub telemetry: String,
    pub water_cmd: String,
    pub water_result: String,
    pub unlock: String,
}

impl Topics {
    pub fn new(device_id: &str) -> Self {
        let base = format!("namo/pilot/{device_id}");
        Self {
            status: format!("{base}/status"),
            telemetry: format!("{base}/telemetry"),
            water_cmd: format!("{base}/water/cmd"),
            water_result: format!("{base}/water/result"),
            unlock: format!("{base}/unlock"),
        }
    }
}

/// 급수 명령 페이로드 (§5.3).
#[derive(Debug, Deserialize)]
struct WaterCommandPayload {
    id: String,
    issued_at: u64,
    ttl_s: u32,
    dose_ml: u32,
}

/// 잠금 해제 페이로드 (§5.1).
#[derive(Debug, Deserialize)]
struct UnlockPayload {
    #[allow(dead_code)]
    id: String,
}

/// Zigbee2MQTT의 누수센서 페이로드. 필요한 필드만 봅니다.
#[derive(Debug, Deserialize)]
struct LeakPayload {
    water_leak: Option<bool>,
}

/// 센서 생존 여부 페이로드. `{"state":"online"}` 형태입니다.
#[derive(Debug, Deserialize)]
struct AvailabilityPayload {
    state: String,
}

/// 클라이언트를 만들고 필요한 토픽을 구독합니다.
///
/// LWT로 `offline`을 걸어둡니다. 전원이 끊기면 브로커가 대신 발행해주므로,
/// 게이트웨이가 장치 생존을 알 수 있습니다.
pub fn connect(
    url: &str,
    client_id: &str,
    topics: &Topics,
) -> Result<(Publisher, EspMqttConnection), EspError> {
    let lwt = LwtConfiguration {
        topic: &topics.status,
        payload: b"offline",
        qos: QoS::AtLeastOnce,
        retain: true,
    };

    let (client, connection) = EspMqttClient::new(
        url,
        &MqttClientConfiguration {
            client_id: Some(client_id),
            lwt: Some(lwt),
            ..Default::default()
        },
    )?;

    Ok((
        Publisher {
            client: Arc::new(Mutex::new(client)),
        },
        connection,
    ))
}

/// 수신 이벤트를 처리합니다. 돌아오지 않습니다.
///
/// 급수 명령은 여기서 실행하지 않고 채널로 넘깁니다. 이 스레드가 펌프를 돌리면
/// 그동안 누수 메시지를 못 받아 중단 판정이 늦어집니다.
pub fn run_event_loop(
    mut connection: EspMqttConnection,
    publisher: Publisher,
    topics: Arc<Topics>,
    shared: Shared,
    commands: std::sync::mpsc::Sender<WaterCommand>,
    on_connect: std::sync::mpsc::Sender<()>,
) {
    while let Ok(event) = connection.next() {
        match event.payload() {
            EventPayload::Connected(_) => {
                log::info!("MQTT 접속됨");
                // 구독은 여기서 하지 않습니다. subscribe는 SUBACK을 기다리고
                // 그 응답은 이 루프가 받아야 하므로, 여기서 부르면 자기가
                // 기다리는 것을 자기가 처리하지 못해 멈춥니다. 클라이언트
                // 락까지 쥔 채로 멈추므로 텔레메트리 발행도 같이 막힙니다.
                if on_connect.send(()).is_err() {
                    log::error!("구독 담당 태스크가 없습니다");
                }
            }
            EventPayload::Disconnected => log::warn!("MQTT 연결 끊김"),
            EventPayload::Received {
                topic: Some(topic),
                data,
                ..
            } => handle_message(topic, data, &topics, &shared, &commands, &publisher),
            _ => {}
        }
    }
    log::warn!("MQTT 이벤트 루프가 끝났습니다");
}

/// 접속될 때마다 구독을 다시 겁니다. 돌아오지 않습니다.
///
/// 이벤트 루프와 **다른 스레드**여야 합니다. 이유는 [`run_event_loop`]의
/// `Connected` 처리에 적어두었습니다.
///
/// 재접속 때도 다시 구독해야 합니다. 브로커가 세션을 유지하지 않으면
/// 구독이 사라지는데, 그러면 급수 명령이 조용히 도착하지 않게 됩니다.
pub fn run_subscriber(
    on_connect: std::sync::mpsc::Receiver<()>,
    publisher: Publisher,
    topics: Arc<Topics>,
) {
    log::info!("MQTT 구독 태스크 시작");
    while on_connect.recv().is_ok() {
        publisher.subscribe(&topics.water_cmd);
        publisher.subscribe(&topics.unlock);
        publisher.subscribe(TOPIC_LEAK_TANK);
        publisher.subscribe(TOPIC_LEAK_POT);
        publisher.subscribe(TOPIC_LEAK_TANK_AVAIL);
        publisher.subscribe(TOPIC_LEAK_POT_AVAIL);
        publisher.publish(&topics.status, b"online", true);
        log::info!("구독 완료. status=online 발행");
    }
    log::warn!("구독 태스크가 끝났습니다");
}

fn handle_message(
    topic: &str,
    data: &[u8],
    topics: &Topics,
    shared: &Shared,
    commands: &std::sync::mpsc::Sender<WaterCommand>,
    publisher: &Publisher,
) {
    if topic == TOPIC_LEAK_TANK || topic == TOPIC_LEAK_POT {
        handle_leak(topic, data, shared);
    } else if topic == TOPIC_LEAK_TANK_AVAIL || topic == TOPIC_LEAK_POT_AVAIL {
        handle_availability(topic, data, shared);
    } else if topic == topics.water_cmd {
        handle_water_cmd(data, commands, shared, publisher, topics);
    } else if topic == topics.unlock {
        handle_unlock(data, shared);
    }
}

fn handle_leak(topic: &str, data: &[u8], shared: &Shared) {
    let payload: LeakPayload = match serde_json::from_slice(data) {
        Ok(v) => v,
        Err(e) => {
            log::warn!("누수 페이로드 파싱 실패 ({topic}): {e}");
            return;
        }
    };

    // Zigbee2MQTT는 배터리 보고처럼 water_leak이 없는 메시지도 보냅니다.
    // 그런 메시지도 "센서가 살아있다"는 신호이므로 시각은 갱신합니다.
    let Some(now) = clock::now() else {
        return;
    };

    let Ok(mut state) = shared.lock() else {
        return;
    };

    let sensor = if topic == TOPIC_LEAK_TANK {
        &mut state.leak_tank
    } else {
        &mut state.leak_pot
    };
    sensor.updated_at = Some(now);
    if let Some(detected) = payload.water_leak {
        sensor.detected = Some(detected);
    }

    // 누수를 보면 즉시 잠급니다(S5). 물이 마른 뒤에도 잠금은 유지되며
    // unlock 명령으로만 풀립니다.
    if payload.water_leak == Some(true) && !state.locked {
        state.locked = true;
        log::error!("누수 감지. 급수를 잠급니다. unlock 명령으로만 해제됩니다.");
    }
}

/// 센서 생존 여부를 반영합니다.
///
/// Zigbee2MQTT는 버전에 따라 `{"state":"online"}` 또는 그냥 `online`을
/// 보냅니다. 둘 다 받습니다.
fn handle_availability(topic: &str, data: &[u8], shared: &Shared) {
    let raw = core::str::from_utf8(data).unwrap_or("").trim();
    let state = match serde_json::from_slice::<AvailabilityPayload>(data) {
        Ok(p) => p.state,
        Err(_) => raw.to_string(),
    };

    let available = match state.as_str() {
        "online" => true,
        "offline" => false,
        other => {
            log::warn!("알 수 없는 availability 값 ({topic}): {other}");
            return;
        }
    };

    let Ok(mut st) = shared.lock() else {
        return;
    };
    let sensor = if topic == TOPIC_LEAK_TANK_AVAIL {
        &mut st.leak_tank
    } else {
        &mut st.leak_pot
    };

    if sensor.available != Some(available) {
        log::info!("{topic}: {}", if available { "online" } else { "offline" });
    }
    sensor.available = Some(available);
}

fn handle_water_cmd(
    data: &[u8],
    commands: &std::sync::mpsc::Sender<WaterCommand>,
    shared: &Shared,
    publisher: &Publisher,
    topics: &Topics,
) {
    let payload: WaterCommandPayload = match serde_json::from_slice(data) {
        Ok(v) => v,
        Err(e) => {
            log::warn!("급수 명령 파싱 실패: {e}");
            return;
        }
    };

    let Some(id) = CommandId::new(&payload.id) else {
        log::warn!("명령 ID가 비었거나 너무 깁니다: {:?}", payload.id);
        return;
    };

    // 급수 중에 들어온 명령은 여기서 바로 거부합니다(§4.6). 워커에게 넘기면
    // 채널에 쌓였다가 앞 급수가 끝난 뒤 실행되는데, 그건 "진행 중이면
    // 거부한다"가 아니라 "진행 중이면 미뤘다가 준다"가 됩니다.
    //
    // 워커도 판정할 때 running을 다시 봅니다. 그 사이 급수가 시작되는
    // 좁은 창이 남지만, 그 경우는 쿨다운(S6)이 막습니다.
    let running = shared
        .lock()
        .map(|state| state.pump == crate::state::PumpState::Running)
        .unwrap_or(true);

    if running {
        log::warn!("명령 {}: 급수 진행 중이라 거부합니다.", payload.id);
        reject(publisher, topics, &payload.id, RejectReason::AlreadyRunning);
        return;
    }

    let command = WaterCommand {
        id,
        issued_at: payload.issued_at,
        ttl_s: payload.ttl_s,
        dose_ml: payload.dose_ml,
    };

    if commands.send(command).is_err() {
        log::error!("펌프 워커가 없습니다. 명령을 버립니다.");
    }
}

/// 거부 결과 페이로드 (§5.4). 워커가 만드는 것과 같은 모양이어야 합니다.
#[derive(Serialize)]
struct RejectResult<'a> {
    id: &'a str,
    status: &'static str,
    reason: &'static str,
    estimated_ml: u32,
    pump_ms: u32,
    started_at: Option<u64>,
    finished_at: Option<u64>,
}

fn reject(publisher: &Publisher, topics: &Topics, id: &str, reason: RejectReason) {
    let now = clock::now();
    let result = RejectResult {
        id,
        status: "rejected",
        reason: reason.as_str(),
        estimated_ml: 0,
        pump_ms: 0,
        started_at: now,
        finished_at: now,
    };
    match serde_json::to_vec(&result) {
        Ok(payload) => publisher.publish(&topics.water_result, &payload, false),
        Err(e) => log::error!("거부 결과 직렬화 실패: {e}"),
    }
}

fn handle_unlock(data: &[u8], shared: &Shared) {
    if serde_json::from_slice::<UnlockPayload>(data).is_err() {
        log::warn!("unlock 페이로드 파싱 실패");
        return;
    }
    let Ok(mut state) = shared.lock() else {
        return;
    };
    if state.locked {
        state.locked = false;
        log::warn!("잠금이 수동 해제됐습니다.");
    }
}
