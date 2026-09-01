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
}
