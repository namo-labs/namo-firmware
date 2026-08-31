# namo-firmware

나모 자택 파일럿의 ESP32-S3 펌웨어와 게이트웨이 구성입니다.

BLE로 화분 센서(HHCC Flower Care)를 읽고, 물통 수위를 감시하고, MOSFET으로 수중 펌프를 구동합니다. 안전 판정은 전부 장치 안에서 하며, 상태 보고와 급수 명령은 MQTT로 주고받습니다.

## 문서

- [설계 스펙](docs/pilot-design.md) — 아키텍처, 핀맵, 안전규칙, MQTT 계약
- [조립·검증 가이드](docs/bring-up-guide.md) — Stage 0~9 절차서
- [하드웨어 현황](docs/hardware-pilot-status.md) — 보유 부품과 미확인 항목
- [추가 구비 목록](docs/shopping-list.md)

## 상태

설계 확정 완료, 펌웨어 구현 미착수.

## 스택

Rust (std, ESP-IDF) · `esp-idf-svc` · `esp32-nimble` · ESP32-S3
