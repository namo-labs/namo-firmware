//! 급수 명령의 TTL과 멱등성.

/// 명령 ID 최대 길이. ULID(26자)와 UUID(36자)를 모두 담습니다.
const MAX_ID_LEN: usize = 36;

/// 멱등성 검사를 위해 기억하는 최근 명령 개수.
pub const RECENT_ID_CAPACITY: usize = 16;

/// 힙 없이 보관하는 명령 ID.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct CommandId {
    bytes: [u8; MAX_ID_LEN],
    len: usize,
}

impl CommandId {
    /// 빈 문자열이거나 최대 길이를 넘으면 `None`입니다.
    pub fn new(s: &str) -> Option<Self> {
        let src = s.as_bytes();
        if src.is_empty() || src.len() > MAX_ID_LEN {
            return None;
        }
        let mut bytes = [0u8; MAX_ID_LEN];
        bytes[..src.len()].copy_from_slice(src);
        Some(Self {
            bytes,
            len: src.len(),
        })
    }

    pub fn as_str(&self) -> &str {
        // `new`에서 &str의 바이트만 받았으므로 UTF-8이 보장됩니다.
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("")
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct WaterCommand {
    pub id: CommandId,
    /// 발행 시각(Unix epoch 초).
    pub issued_at: u64,
    pub ttl_s: u32,
    pub dose_ml: u32,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum TtlVerdict {
    Fresh,
    /// 실행하지 않고 조용히 버립니다. 결과도 발행하지 않습니다.
    Expired,
}

/// 명령이 아직 유효한지 판정합니다.
///
/// `max_future_skew_s`는 시계 오차로 발행시각이 미래로 보일 때 허용할 폭입니다.
/// 이 폭을 넘어 미래면 시각을 신뢰할 수 없으므로 만료로 봅니다(fail-closed).
pub fn check_ttl(cmd: &WaterCommand, now: u64, max_future_skew_s: u64) -> TtlVerdict {
    if cmd.issued_at > now {
        return if cmd.issued_at - now <= max_future_skew_s {
            TtlVerdict::Fresh
        } else {
            TtlVerdict::Expired
        };
    }
    if now - cmd.issued_at <= cmd.ttl_s as u64 {
        TtlVerdict::Fresh
    } else {
        TtlVerdict::Expired
    }
}

/// 최근 처리한 명령 ID를 담는 링버퍼.
#[derive(Debug)]
pub struct RecentIds {
    ids: [Option<CommandId>; RECENT_ID_CAPACITY],
    next: usize,
}

impl Default for RecentIds {
    fn default() -> Self {
        Self::new()
    }
}

impl RecentIds {
    pub fn new() -> Self {
        Self {
            ids: [None; RECENT_ID_CAPACITY],
            next: 0,
        }
    }

    /// 처음 보는 ID면 기록하고 `true`, 이미 본 ID면 `false`입니다.
    pub fn record(&mut self, id: &CommandId) -> bool {
        if self.ids.iter().flatten().any(|seen| seen == id) {
            return false;
        }
        self.ids[self.next] = Some(*id);
        self.next = (self.next + 1) % RECENT_ID_CAPACITY;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn 명령(id: &str, issued_at: u64, ttl_s: u32) -> WaterCommand {
        WaterCommand {
            id: CommandId::new(id).unwrap(),
            issued_at,
            ttl_s,
            dose_ml: 100,
        }
    }

    #[test]
    fn 유효기간_안이면_신선하다() {
        let c = 명령("cmd-1", 1000, 15);
        assert_eq!(check_ttl(&c, 1010, 5), TtlVerdict::Fresh);
    }

    #[test]
    fn 경계값은_아직_신선하다() {
        let c = 명령("cmd-1", 1000, 15);
        assert_eq!(check_ttl(&c, 1015, 5), TtlVerdict::Fresh);
    }

    #[test]
    fn 유효기간을_넘으면_만료다() {
        let c = 명령("cmd-1", 1000, 15);
        assert_eq!(check_ttl(&c, 1016, 5), TtlVerdict::Expired);
    }

    #[test]
    fn 브로커_재연결로_한참_뒤에_도착하면_만료다() {
        let c = 명령("cmd-1", 1000, 15);
        assert_eq!(check_ttl(&c, 4000, 5), TtlVerdict::Expired);
    }

    #[test]
    fn 약간_미래인_발행시각은_허용한다() {
        // 클럭 스큐로 발행시각이 살짝 미래일 수 있습니다.
        let c = 명령("cmd-1", 1003, 15);
        assert_eq!(check_ttl(&c, 1000, 5), TtlVerdict::Fresh);
    }

    #[test]
    fn 크게_미래인_발행시각은_거부한다() {
        // 시각이 크게 어긋난 상태를 신뢰하면 TTL이 무의미해집니다.
        let c = 명령("cmd-1", 2000, 15);
        assert_eq!(check_ttl(&c, 1000, 5), TtlVerdict::Expired);
    }

    #[test]
    fn 처음_보는_아이디는_기록된다() {
        let mut r = RecentIds::new();
        assert!(r.record(&CommandId::new("cmd-1").unwrap()));
    }

    #[test]
    fn 같은_아이디는_두_번째부터_거부된다() {
        let mut r = RecentIds::new();
        let id = CommandId::new("cmd-1").unwrap();
        assert!(r.record(&id));
        assert!(!r.record(&id));
        assert!(!r.record(&id));
    }

    #[test]
    fn 서로_다른_아이디는_각각_통과한다() {
        let mut r = RecentIds::new();
        assert!(r.record(&CommandId::new("cmd-1").unwrap()));
        assert!(r.record(&CommandId::new("cmd-2").unwrap()));
    }

    #[test]
    fn 용량을_넘기면_가장_오래된_기록이_밀려난다() {
        let mut r = RecentIds::new();
        for i in 0..RECENT_ID_CAPACITY {
            let mut buf = [0u8; 8];
            let s = fmt_id(&mut buf, i);
            assert!(r.record(&CommandId::new(s).unwrap()));
        }
        // 하나 더 넣으면 첫 번째가 밀려납니다.
        assert!(r.record(&CommandId::new("overflow").unwrap()));

        let mut buf = [0u8; 8];
        let 첫번째 = fmt_id(&mut buf, 0);
        assert!(r.record(&CommandId::new(첫번째).unwrap()));
    }

    /// 테스트용 소형 정수 → 문자열 변환 (no_std라 format! 없음).
    fn fmt_id(buf: &mut [u8; 8], n: usize) -> &str {
        buf[0] = b'i';
        buf[1] = b'0' + (n / 10) as u8;
        buf[2] = b'0' + (n % 10) as u8;
        core::str::from_utf8(&buf[..3]).unwrap()
    }

    #[test]
    fn 너무_긴_아이디는_거부한다() {
        let long = "0123456789012345678901234567890123456789";
        assert!(CommandId::new(long).is_none());
    }

    #[test]
    fn 빈_아이디는_거부한다() {
        assert!(CommandId::new("").is_none());
    }

    #[test]
    fn 아이디_문자열을_되돌려준다() {
        let id = CommandId::new("01JBQ8Z4K2N7XW").unwrap();
        assert_eq!(id.as_str(), "01JBQ8Z4K2N7XW");
    }
}
