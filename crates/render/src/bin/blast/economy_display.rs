//! User-facing units only: the physical ledger keeps its exact integer mass.
const MASS_UNIT: u64 = 100_000;
const POWER_UNIT: u64 = 10;

fn amount(raw: u64, unit: u64) -> String {
    if raw == 0 { return "0".into(); }
    if u128::from(raw) * 200 < u128::from(unit) { return "<0,01".into(); }
    let decimals = if raw < unit.saturating_mul(10) { 2 }
        else if raw < unit.saturating_mul(1000) { 1 } else { 0 };
    let factor = 10u128.pow(decimals);
    let scaled = (u128::from(raw) * factor + u128::from(unit) / 2) / u128::from(unit);
    let integer = (scaled / factor).to_string();
    let mut grouped = String::with_capacity(integer.len() + integer.len() / 3);
    for (index, ch) in integer.chars().enumerate() {
        if index > 0 && (integer.len() - index) % 3 == 0 { grouped.push(' '); }
        grouped.push(ch);
    }
    if decimals == 0 { return grouped; }
    let fraction = format!("{:0width$}", scaled % factor, width = decimals as usize);
    let fraction = fraction.trim_end_matches('0');
    if !fraction.is_empty() { grouped.push(','); grouped.push_str(fraction); }
    grouped
}

pub fn mass(raw: u64) -> String { amount(raw, MASS_UNIT) }
pub fn energy(raw: u64) -> String { amount(raw, POWER_UNIT) }
/// Integrate power-ticks into power-seconds at the simulation tick rate.
pub fn energy_work(raw: u64) -> String { amount(raw, POWER_UNIT * sim_core::TICKS_PER_SECOND as u64) }

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn consistent_units_keep_small_and_large_values_readable() {
        assert_eq!(mass(0), "0");
        assert_eq!(mass(785), "0,01");
        assert_eq!(mass(10_330_600), "103,3");
        assert_eq!(mass(320_933_120), "3 209");
        assert_eq!(mass(172_700), "1,73");
        assert_eq!(energy(600), "60");
        assert_eq!(energy(1000), "100");
    }
}
