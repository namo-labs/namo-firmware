//! Xiaomi MiBeacon 광고 파싱.

/// HHCC Flower Care(HHCCJCY01)의 MiBeacon 디바이스 타입.
pub const HHCC_DEVICE_TYPE: u16 = 0x0098;

const FLAG_ENCRYPTED: u8 = 0x08;
const FLAG_CAPABILITY: u8 = 0x20;
const FLAG_HAS_DATA: u8 = 0x40;

/// 헤더 최소 길이. 플래그 2 + 디바이스타입 2 + 카운터 1 + MAC 6 = 11.
const HEADER_LEN: usize = 11;

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum ParseError {
    /// 헤더를 채울 만큼도 되지 않습니다.
    TooShort,
    /// 데이터 플래그가 꺼져 있어 측정값이 실려 있지 않습니다.
    NoData,
    /// 암호화된 페이로드입니다. bindkey 없이 읽을 수 없습니다.
    Encrypted,
    /// HHCC가 아닌 다른 Xiaomi 기기입니다.
    NotHhcc,
    /// 헤더는 유효하나 payload가 잘렸습니다.
    Truncated,
    /// payload에 해석 가능한 측정값이 하나도 없습니다.
    NoMeasurement,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct Header {
    pub device_type: u16,
    pub frame_counter: u8,
    /// 사람이 읽는 순서(광고에는 역순으로 실려 옵니다).
    pub mac: [u8; 6],
    pub payload_offset: usize,
}

/// 서비스 데이터(UUID 0xFE95)의 헤더를 해석합니다.
///
/// 호출자가 서비스 UUID를 이미 걸렀다고 가정합니다.
pub fn parse_header(raw: &[u8]) -> Result<Header, ParseError> {
    if raw.len() < HEADER_LEN {
        return Err(ParseError::TooShort);
    }

    let flags = raw[0];
    if flags & FLAG_HAS_DATA == 0 {
        return Err(ParseError::NoData);
    }
    if flags & FLAG_ENCRYPTED != 0 {
        return Err(ParseError::Encrypted);
    }

    let device_type = u16::from_le_bytes([raw[2], raw[3]]);
    if device_type != HHCC_DEVICE_TYPE {
        return Err(ParseError::NotHhcc);
    }

    let payload_offset = if flags & FLAG_CAPABILITY != 0 { 12 } else { 11 };
    if payload_offset >= raw.len() {
        return Err(ParseError::Truncated);
    }

    let mut mac = [0u8; 6];
    for (i, slot) in mac.iter_mut().enumerate() {
        *slot = raw[10 - i];
    }

    Ok(Header {
        device_type,
        frame_counter: raw[4],
        mac,
        payload_offset,
    })
}

/// 한 광고에 담길 수 있는 측정값 개수 상한.
/// HHCC는 보통 1개만 보내지만 연속 데이터포인트를 허용합니다.
const MAX_MEASUREMENTS: usize = 4;

const TYPE_TEMPERATURE: u16 = 0x1004;
const TYPE_ILLUMINANCE: u16 = 0x1007;
const TYPE_MOISTURE: u16 = 0x1008;
const TYPE_CONDUCTIVITY: u16 = 0x1009;
const TYPE_BATTERY: u16 = 0x100A;

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Measurement {
    /// 0.1℃ 단위. 224 == 22.4℃.
    TemperatureDeciC(i16),
    IlluminanceLux(u32),
    MoisturePct(u8),
    ConductivityUsCm(u16),
    BatteryPct(u8),
}

/// 한 광고에서 뽑은 측정값 모음. 힙 없이 고정 배열로 담습니다.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct Measurements {
    items: [Measurement; MAX_MEASUREMENTS],
    len: usize,
}

impl Measurements {
    fn new() -> Self {
        Self {
            items: [Measurement::MoisturePct(0); MAX_MEASUREMENTS],
            len: 0,
        }
    }

    fn push(&mut self, m: Measurement) {
        if self.len < MAX_MEASUREMENTS {
            self.items[self.len] = m;
            self.len += 1;
        }
    }

    pub fn as_slice(&self) -> &[Measurement] {
        &self.items[..self.len]
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

/// 고정바이트로 허용되는 값. 이게 아니면 잔여 데이터로 보고 파싱을 멈춥니다.
fn is_fixed_byte(b: u8) -> bool {
    matches!(b, 0x10 | 0x00 | 0x4C | 0x48)
}

/// 데이터포인트 하나를 해석합니다. 알 수 없는 타입이나 길이 불일치는 `None`입니다.
fn decode_value(value_type: u16, data: &[u8]) -> Option<Measurement> {
    match (value_type, data.len()) {
        (TYPE_TEMPERATURE, 2) => Some(Measurement::TemperatureDeciC(i16::from_le_bytes([
            data[0], data[1],
        ]))),
        (TYPE_ILLUMINANCE, 3) => Some(Measurement::IlluminanceLux(u32::from_le_bytes([
            data[0], data[1], data[2], 0,
        ]))),
        (TYPE_MOISTURE, 1) => Some(Measurement::MoisturePct(data[0])),
        (TYPE_CONDUCTIVITY, 2) => Some(Measurement::ConductivityUsCm(u16::from_le_bytes([
            data[0], data[1],
        ]))),
        (TYPE_BATTERY, 1) => Some(Measurement::BatteryPct(data[0])),
        _ => None,
    }
}

/// HHCC 서비스 데이터를 헤더와 측정값으로 해석합니다.
pub fn parse(raw: &[u8]) -> Result<(Header, Measurements), ParseError> {
    let header = parse_header(raw)?;
    let payload = &raw[header.payload_offset..];

    let mut out = Measurements::new();
    let mut cursor = 0usize;

    // 데이터포인트 최소 크기는 타입2 + 길이1 + 값1 = 4바이트입니다.
    while payload.len() >= cursor + 4 {
        if !is_fixed_byte(payload[cursor + 1]) {
            break;
        }

        let value_len = payload[cursor + 2] as usize;
        if !(1..=4).contains(&value_len) {
            break;
        }
        let value_end = cursor + 3 + value_len;
        if value_end > payload.len() {
            break;
        }

        let value_type = u16::from_le_bytes([payload[cursor], payload[cursor + 1]]);
        if let Some(m) = decode_value(value_type, &payload[cursor + 3..value_end]) {
            out.push(m);
        }

        cursor = value_end;
    }

    if out.is_empty() {
        return Err(ParseError::NoMeasurement);
    }
    Ok((header, out))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// capability 플래그가 있는 HHCC 수분 광고. payload는 인덱스 12부터.
    const 수분_32퍼센트: [u8; 16] = [
        0x71, 0x20, 0x98, 0x00, 0x0C, 0xEC, 0x9C, 0x8B, 0x6C, 0x7C, 0xC4, 0x00,
        0x08, 0x10, 0x01, 0x20,
    ];

    /// capability 플래그가 없는 광고. payload는 인덱스 11부터.
    const 수분_21퍼센트_CAP없음: [u8; 15] = [
        0x51, 0x20, 0x98, 0x00, 0x10, 0xEC, 0x9C, 0x8B, 0x6C, 0x7C, 0xC4,
        0x08, 0x10, 0x01, 0x15,
    ];

    #[test]
    fn capability가_있으면_payload는_12부터() {
        let h = parse_header(&수분_32퍼센트).unwrap();
        assert_eq!(h.payload_offset, 12);
        assert_eq!(h.device_type, HHCC_DEVICE_TYPE);
        assert_eq!(h.frame_counter, 0x0C);
    }

    #[test]
    fn capability가_없으면_payload는_11부터() {
        let h = parse_header(&수분_21퍼센트_CAP없음).unwrap();
        assert_eq!(h.payload_offset, 11);
    }

    #[test]
    fn mac은_역순으로_복원된다() {
        let h = parse_header(&수분_32퍼센트).unwrap();
        assert_eq!(h.mac, [0xC4, 0x7C, 0x6C, 0x8B, 0x9C, 0xEC]);
    }

    #[test]
    fn 암호화된_프레임은_거부한다() {
        let mut raw = 수분_32퍼센트;
        raw[0] |= 0x08;
        assert_eq!(parse_header(&raw), Err(ParseError::Encrypted));
    }

    #[test]
    fn 데이터_플래그가_없으면_거부한다() {
        let mut raw = 수분_32퍼센트;
        raw[0] &= !0x40;
        assert_eq!(parse_header(&raw), Err(ParseError::NoData));
    }

    #[test]
    fn 다른_기기는_거부한다() {
        let mut raw = 수분_32퍼센트;
        raw[2] = 0xAA; // 0x01AA = LYWSDCGQ 온습도계
        raw[3] = 0x01;
        assert_eq!(parse_header(&raw), Err(ParseError::NotHhcc));
    }

    #[test]
    fn 너무_짧으면_거부한다() {
        assert_eq!(parse_header(&[0x71, 0x20, 0x98]), Err(ParseError::TooShort));
    }

    #[test]
    fn payload_오프셋이_길이를_넘으면_거부한다() {
        // 헤더만 있고 payload가 없는 12바이트 프레임
        let raw = [0x71, 0x20, 0x98, 0x00, 0x0C, 0xEC, 0x9C, 0x8B, 0x6C, 0x7C, 0xC4, 0x00];
        assert_eq!(parse_header(&raw), Err(ParseError::Truncated));
    }

    /// 온도 22.4℃ (224 = 0x00E0)
    const 온도_22_4도: [u8; 17] = [
        0x71, 0x20, 0x98, 0x00, 0x0D, 0xEC, 0x9C, 0x8B, 0x6C, 0x7C, 0xC4, 0x00,
        0x04, 0x10, 0x02, 0xE0, 0x00,
    ];

    /// 영하 3.5℃ (-35 = 0xFFDD)
    const 온도_영하3_5도: [u8; 17] = [
        0x71, 0x20, 0x98, 0x00, 0x13, 0xEC, 0x9C, 0x8B, 0x6C, 0x7C, 0xC4, 0x00,
        0x04, 0x10, 0x02, 0xDD, 0xFF,
    ];

    /// 조도 1820 lux (0x00071C)
    const 조도_1820: [u8; 18] = [
        0x71, 0x20, 0x98, 0x00, 0x0E, 0xEC, 0x9C, 0x8B, 0x6C, 0x7C, 0xC4, 0x00,
        0x07, 0x10, 0x03, 0x1C, 0x07, 0x00,
    ];

    /// 전도도 340 µS/cm (0x0154)
    const 전도도_340: [u8; 17] = [
        0x71, 0x20, 0x98, 0x00, 0x0F, 0xEC, 0x9C, 0x8B, 0x6C, 0x7C, 0xC4, 0x00,
        0x09, 0x10, 0x02, 0x54, 0x01,
    ];

    /// 수분 32% + 전도도 340 µS/cm 이 한 프레임에 연속으로 들어온 경우
    const 수분과_전도도: [u8; 21] = [
        0x71, 0x20, 0x98, 0x00, 0x11, 0xEC, 0x9C, 0x8B, 0x6C, 0x7C, 0xC4, 0x00,
        0x08, 0x10, 0x01, 0x20,
        0x09, 0x10, 0x02, 0x54, 0x01,
    ];

    fn 측정값_하나(raw: &[u8]) -> Measurement {
        let (_, ms) = parse(raw).unwrap();
        assert_eq!(ms.as_slice().len(), 1);
        ms.as_slice()[0]
    }

    #[test]
    fn 수분을_읽는다() {
        assert_eq!(측정값_하나(&수분_32퍼센트), Measurement::MoisturePct(32));
    }

    #[test]
    fn capability없는_프레임의_수분을_읽는다() {
        assert_eq!(측정값_하나(&수분_21퍼센트_CAP없음), Measurement::MoisturePct(21));
    }

    #[test]
    fn 온도를_0_1도_단위로_읽는다() {
        assert_eq!(측정값_하나(&온도_22_4도), Measurement::TemperatureDeciC(224));
    }

    #[test]
    fn 영하_온도를_읽는다() {
        assert_eq!(측정값_하나(&온도_영하3_5도), Measurement::TemperatureDeciC(-35));
    }

    #[test]
    fn 조도를_읽는다() {
        assert_eq!(측정값_하나(&조도_1820), Measurement::IlluminanceLux(1820));
    }

    #[test]
    fn 전도도를_읽는다() {
        assert_eq!(측정값_하나(&전도도_340), Measurement::ConductivityUsCm(340));
    }

    #[test]
    fn 한_프레임의_측정값_두_개를_모두_읽는다() {
        let (_, ms) = parse(&수분과_전도도).unwrap();
        assert_eq!(
            ms.as_slice(),
            &[Measurement::MoisturePct(32), Measurement::ConductivityUsCm(340)]
        );
    }

    #[test]
    fn 고정바이트가_틀리면_측정값이_없다() {
        let mut raw = 수분_32퍼센트;
        raw[13] = 0x99; // 허용 고정바이트(0x10/0x00/0x4C/0x48)가 아님
        assert_eq!(parse(&raw), Err(ParseError::NoMeasurement));
    }

    #[test]
    fn 길이가_맞지_않는_데이터포인트는_무시한다() {
        let mut raw = 수분_32퍼센트;
        raw[14] = 0x02; // 수분은 1바이트인데 2바이트라고 주장
        assert_eq!(parse(&raw), Err(ParseError::NoMeasurement));
    }

    #[test]
    fn 알_수_없는_타입은_건너뛰고_다음을_읽는다() {
        // 앞에 알 수 없는 타입(0x1099, 1바이트)을 붙이고 뒤에 수분을 둡니다.
        let raw: [u8; 20] = [
            0x71, 0x20, 0x98, 0x00, 0x12, 0xEC, 0x9C, 0x8B, 0x6C, 0x7C, 0xC4, 0x00,
            0x99, 0x10, 0x01, 0x00,
            0x08, 0x10, 0x01, 0x20,
        ];
        assert_eq!(측정값_하나(&raw), Measurement::MoisturePct(32));
    }
}
