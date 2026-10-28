//! Shared fixed-step clock. Stream sample units retain their authored 30 Hz
//! resolution so fast salvos do not lose shots when the simulation runs slower.
const fn parse_rate(text: &str) -> u32 {
    let bytes = text.as_bytes();
    let mut value = 0;
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c >= b'0' && c <= b'9' {
            value = value * 10 + (c - b'0') as u32;
        } else {
            assert!(c == b'\n' || c == b'\r' || c == b' ');
        }
        i += 1;
    }
    assert!(value == 20 || value == 30);
    value
}
pub const HZ: u32 = parse_rate(include_str!("../../../data/tick-rate.ron"));
pub const STREAM_HZ: u32 = 30;
/// Rounded deadline, never earlier than the authored sample time.
pub const fn stream_ticks(samples: u32) -> u32 {
    (samples as u64 * HZ as u64).div_ceil(STREAM_HZ as u64) as u32
}
pub const fn stream_time(tick: u32) -> u64 {
    tick as u64 * STREAM_HZ as u64 / HZ as u64
}
pub const fn stream_samples(tick: u32) -> u32 {
    (stream_time(tick + 1) - stream_time(tick)) as u32
}
/// Integrate a legacy per-tick integer rate without losing fractional units.
pub const fn rate(value: u32, tick: u32) -> u32 {
    let numerator = value as u64 * STREAM_HZ as u64;
    (((tick as u64 + 1) * numerator / HZ as u64) - (tick as u64 * numerator / HZ as u64)) as u32
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn one_second_keeps_thirty_samples_and_fifteen_salvos() {
        assert_eq!((0..HZ).map(stream_samples).sum::<u32>(), STREAM_HZ);
        let mut next = 0;
        let mut salvos = 0;
        for tick in 0..HZ {
            while stream_time(tick) >= next {
                salvos += 1;
                next += 2;
            }
        }
        assert_eq!(salvos, 15);
        assert_eq!(stream_ticks(90), HZ * 3);
    }
}
