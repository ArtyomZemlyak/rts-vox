//! Finite material energy uses the same authored body/ground split as Bursts.
use super::{matter, Battle, BlastRange, Explosion, ExplosionConfig, RangeUnit};
use fx::blob::{Blob, Scan};

impl Explosion {
    /// Material blast entry point. Pressure-only fields keep using `begin`.
    /// Bodies outside the frozen domain do not reserve fracture energy.
    pub fn begin_material(
        at: [i32; 3],
        config: &ExplosionConfig,
        units: &[RangeUnit],
        body_energy_pct: u32,
    ) -> Self {
        let mut e = Self::begin(at, config, units);
        let end = e.origin.map(|v| v + e.wave.size() as i32 * e.cell);
        if config.material_energy_j > 0
            && units.iter().any(|u| {
                let (lo, hi) = u.bounds();
                (0..3).all(|a| lo[a] < end[a] && hi[a] >= e.origin[a])
            })
        {
            e.body_energy_mj = Some(
                u64::from(config.material_energy_j) * 1000 * u64::from(body_energy_pct.min(100))
                    / 100,
            );
        }
        e
    }

    /// Replace the finite payload while retaining frozen receiver eligibility.
    /// No new energy is created by claims; the caller supplies the same total
    /// to Matter and Burst, and the reservation is serialized with the wave.
    pub(super) fn material_budget(&mut self, energy_mj: u64, body_energy_pct: u32) {
        if self.body_energy_mj.is_some() {
            self.body_energy_mj = Some(energy_mj / 100 * u64::from(body_energy_pct.min(100)));
        }
    }

    /// Expose only the receiver's share to existing paid claims. No energy is
    /// supplied here; failed/deferred claims spend nothing.
    pub(crate) fn claim_energy(
        &mut self,
        matter: &mut matter::Matter,
        body: bool,
        claim: impl FnOnce(&mut matter::Matter) -> matter::ClaimResult,
    ) -> matter::ClaimResult {
        let Some(reserved) = self.body_energy_mj else {
            return claim(matter);
        };
        let total = matter.remaining_mj;
        let available = if body {
            reserved.min(total)
        } else {
            total.saturating_sub(reserved)
        };
        matter.remaining_mj = available;
        let result = claim(matter);
        let spent = available - matter.remaining_mj;
        matter.remaining_mj = total - spent;
        if body {
            self.body_energy_mj = Some(reserved - spent);
        }
        result
    }

    pub(crate) fn save_energy(&self, b: &mut Blob) {
        b.bool(self.body_energy_mj.is_some());
        if let Some(energy) = self.body_energy_mj {
            b.u64(energy);
        }
    }
    pub(crate) fn load_energy(&mut self, s: &mut Scan, total: u64) -> Option<()> {
        self.body_energy_mj = match s.u8() {
            0 => None,
            1 => {
                let energy = s.u64();
                if energy > total {
                    return None;
                }
                Some(energy)
            }
            _ => return None,
        };
        s.ok().then_some(())
    }
}

impl Battle {
    pub(crate) fn has_material_work(&self) -> bool {
        self.waves.iter().any(|w| w.energy > 0 && w.wave.working())
    }
    pub(crate) fn has_wave_energy(&self) -> bool {
        self.waves.iter().any(|w| w.wave.body_energy_mj.is_some())
    }
    pub(crate) fn save_wave_energy(&self, b: &mut Blob) {
        for w in &self.waves {
            w.wave.save_energy(b);
        }
    }
    pub(crate) fn load_wave_energy(&mut self, s: &mut Scan) -> Option<()> {
        for w in &mut self.waves {
            w.wave.load_energy(s, w.energy)?;
        }
        Some(())
    }
}

impl BlastRange {
    /// Physical debris budgets are soft in autonomous battles. Once a pool
    /// is saturated while real work is waiting, grow it deterministically;
    /// parked rubble and live fragments remain intact and round-trip in the
    /// existing debris-tuning snapshot. The normal path pays no extra scan.
    pub(crate) fn relieve_autonomous_damage_pressure(&mut self) {
        if !self.battle_mode()
            || !self.battle.autonomous
            || self.tick % 10 != 0
            || !(self.matter.felling_pending() > 0
                || self.pending_kinetic_impacts() > 0
                || self.battle.has_material_work()
                || self.thermals.hot() > 0)
        {
            return;
        }
        let pools = self.matter.debris.fragment_pools(&self.matter.tuning);
        let fragments = self.matter.debris.fragments.len();
        let reserved = self.matter.debris.reserved();
        let claims = self.matter.debris.claims();
        let t = &mut self.matter.tuning;
        let grow = |value: u32| value.saturating_add((value / 4).max(256));
        if pools.parked >= t.max_resting_fragments as usize && fragments > pools.parked {
            t.max_resting_fragments = grow(t.max_resting_fragments);
        }
        if pools.flying.saturating_add(reserved) >= t.max_flying as usize {
            t.max_flying = grow(t.max_flying);
        }
        if pools.working.saturating_add(reserved) >= t.max_fragments as usize {
            t.max_fragments = grow(t.max_fragments);
        }
        if claims >= t.max_claims as usize {
            t.max_claims = grow(t.max_claims);
        }
        t.max_fragments = t.max_fragments.max(t.max_flying);
        t.max_resting_fragments = t.max_resting_fragments.min(u32::MAX - t.max_fragments);
    }
    pub(crate) fn reserve_pending_damage(&mut self) {
        let waiting = self.battle.has_material_work()
            || self.pending_kinetic_impacts() > 0
            || (self.config.material_energy_j > 0
                && self.matter.remaining_mj > 0
                && self.explosion.as_ref().is_some_and(|e| e.working()));
        self.matter.reserve_damage(waiting, u64::from(self.tick));
    }
    pub(crate) fn has_wave_energy(&self) -> bool {
        self.explosion
            .as_ref()
            .is_some_and(|e| e.body_energy_mj.is_some())
            || self.battle.has_wave_energy()
    }
    pub(crate) fn save_wave_energy(&self, b: &mut Blob) {
        if let Some(e) = &self.explosion {
            e.save_energy(b);
        }
        self.battle.save_wave_energy(b);
    }
    pub(crate) fn load_wave_energy(&mut self, s: &mut Scan) -> Option<()> {
        if let Some(e) = &mut self.explosion {
            e.load_energy(s, self.matter.remaining_mj)?;
        }
        self.battle.load_wave_energy(s)
    }
}
