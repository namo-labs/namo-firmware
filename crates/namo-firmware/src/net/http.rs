//! 상태 페이지. 스펙 `docs/pilot-design.md` §4.4의 `http_server`.
//!
//! MQTT 없이 브라우저만으로 상태를 볼 수 있게 하는 것이 목적입니다.
//! 브로커가 죽었거나 게이트웨이를 안 켰을 때 장치가 살아있는지 확인하는
//! 가장 빠른 방법입니다.
//!
//! **읽기 전용입니다.** 급수 명령은 받지 않습니다. 인증 없는 HTTP로 펌프를
//! 켤 수 있게 하면 같은 네트워크의 누구나 물을 줄 수 있습니다.

use esp_idf_svc::http::server::{Configuration, EspHttpServer};
use esp_idf_svc::http::Method;
use esp_idf_svc::io::{EspIOError, Write};

use crate::state::Shared;
use crate::telemetry;

const PAGE: &str = r#"<!doctype html>
<html lang="ko">
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>나모 파일럿</title>
<style>
  body { font-family: -apple-system, system-ui, sans-serif; margin: 2rem auto;
         max-width: 32rem; padding: 0 1rem; line-height: 1.6; }
  h1 { font-size: 1.25rem; }
  table { border-collapse: collapse; width: 100%; }
  th, td { text-align: left; padding: .4rem .6rem; border-bottom: 1px solid #ddd; }
  th { width: 45%; font-weight: 500; color: #666; }
  .warn { color: #b00; font-weight: 600; }
  footer { margin-top: 1.5rem; color: #888; font-size: .85rem; }
</style>
<h1>나모 파일럿</h1>
<table id="t"></table>
<footer>2초마다 갱신됩니다. 이 페이지는 읽기 전용입니다.</footer>
<script>
const LABELS = {
  reservoir: "물통", leak: "누수", pump: "펌프",
  soil_moisture_pct: "흙 수분 (%)", soil_temperature_c: "온도 (℃)",
  illuminance_lux: "조도 (lux)", soil_conductivity_us_cm: "전도도 (µS/cm)",
  sensor_seen_ago_s: "센서 수신 경과 (초)", leak_seen_ago_s: "누수 수신 경과 (초)",
  leak_sensors_online: "누수센서 생존",
  today_estimated_ml: "오늘 급수 (mL)", locked: "잠김", ts: "장치 시각"
};
async function tick() {
  try {
    const s = await (await fetch("/api/state")).json();
    document.getElementById("t").innerHTML = Object.entries(LABELS)
      .map(([k, label]) => {
        let v = s[k];
        if (v === null || v === undefined) v = "—";
        const bad = (k === "leak" && v === "detected") || (k === "locked" && v === true)
                 || (k === "reservoir" && v !== "ok") || (k === "leak_sensors_online" && v === false);
        return `<tr><th>${label}</th><td class="${bad ? "warn" : ""}">${v}</td></tr>`;
      }).join("");
  } catch (e) { /* 갱신 실패는 다음 주기에 다시 시도합니다 */ }
}
tick(); setInterval(tick, 2000);
</script>
"#;

/// HTTP 서버를 띄웁니다.
///
/// 반환된 핸들을 살려둬야 서버가 계속 돕니다. 떨어뜨리면 즉시 내려갑니다.
pub fn start(shared: Shared) -> Result<EspHttpServer<'static>, EspIOError> {
    // 기본 스택으로는 핸들러 안에서 JSON을 만들다 넘칠 수 있습니다.
    let mut server = EspHttpServer::new(&Configuration {
        stack_size: 8192,
        ..Default::default()
    })?;

    server.fn_handler("/", Method::Get, |request| {
        request
            .into_response(200, None, &[("Content-Type", "text/html; charset=utf-8")])?
            .write_all(PAGE.as_bytes())
    })?;

    server.fn_handler("/api/state", Method::Get, move |request| {
        let body = match telemetry::snapshot(&shared) {
            Some(state) => serde_json::to_vec(&state)
                .unwrap_or_else(|_| br#"{"error":"serialize"}"#.to_vec()),
            None => br#"{"error":"state_lock"}"#.to_vec(),
        };
        request
            .into_response(200, None, &[("Content-Type", "application/json")])?
            .write_all(&body)
    })?;

    log::info!("HTTP 상태 페이지가 80번 포트에서 돕니다");
    Ok(server)
}
