#![no_std]
#![forbid(unsafe_code)]

pub mod command;
pub mod daily;
pub mod dose;
pub mod float;
pub mod mibeacon;
pub mod safety;
pub mod telemetry;

#[cfg(test)]
mod tests {
    #[test]
    fn 크레이트가_빌드된다() {
        assert_eq!(crate::mibeacon::HHCC_DEVICE_TYPE, 0x0098);
    }
}
