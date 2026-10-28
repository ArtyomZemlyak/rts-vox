//! Conservative exchange with bed-load transport. No sediment particles.
use super::*;
impl Water {
    pub(in crate::blast) fn saturation_milli(&self, p: Pos, mat: u16) -> u32 {
        self.wet
            .get(&p)
            .filter(|v| v.0 == mat)
            .and_then(|v| {
                self.tuning
                    .porous
                    .iter()
                    .find(|m| m.material == mat)
                    .map(|m| (u64::from(v.1) * 1000 / u64::from(m.capacity)) as u32)
            })
            .unwrap_or(0)
    }
    pub(in crate::blast) fn can_exchange_grain(&self, source: Pos, target: Pos) -> bool {
        // Source is still solid. Do not merge an older occluded fluid job.
        self.amount(source) == 0 && !self.wet.contains_key(&target)
    }
    pub(in crate::blast) fn exchange_grain(&mut self, source: Pos, target: Pos, size: Pos) {
        let amount = self.amount(target);
        let thermal=self.take_moving_heat(target,amount);
        self.set(target, 0);
        self.set(source, amount);
        if thermal>0 {self.absorb_heat(source,thermal,size);}
        self.wake_near(source, size);
        self.wake_near(target, size);
        // Initial/free totals stay exact; displacement is not a new flux event.
        // Pore water follows the existing source-release path on the next step.
    }
}
#[cfg(test)]
#[path = "range_erosion_tests.rs"]
mod tests;
