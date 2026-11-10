//! Finite engine-compartment fuel. Matter is consumed only by Combustion.
//! A breached reservoir becomes a bounded point spill, not a liquid solver.
use super::{
    matter::Matter,
    thermal::{temperature, Cell},
    BlastRange, RangeUnit,
};
use crate::matter::{Kind, Sink};
use fx::{
    blob::{Blob, Scan},
    Fx,
};
use modding::range_fire::Tuning;
use std::collections::BTreeSet;
use voxel::unit::{Part, WORLD_MM};

fn fuel_cell(kind: modding::range::RangeUnitKind, at: [u8; 3]) -> [u8; 3] {
    if kind == modding::range::RangeUnitKind::Crawler {
        at.map(|v| v * 3 + 1)
    } else if kind == modding::range::RangeUnitKind::LightningTank {
        at.map(|v| v * 3 / 2)
    } else if kind == modding::range::RangeUnitKind::Commander {
        [66, 54, 72]
    } else if kind == modding::range::RangeUnitKind::Walker04 {
        [37, 30, 40]
    } else if kind == modding::range::RangeUnitKind::SkobaDrone {
        [120, 16, 31]
    } else {
        at
    }
}

#[derive(Clone)]
struct Spill {
    at: [Fx; 3],
    velocity: [Fx; 3],
}

#[derive(Clone, Default)]
struct Stock {
    held: Option<u32>,
    burning: bool,
    spill: Option<Spill>,
}
pub struct Combustion {
    stocks: Vec<Stock>,
    pub tuning: Tuning,
    pub consumed: u64,
    pub generated_heat: u64,
    pub dissipated_heat: u64,
}
#[derive(Clone, Copy)]
pub struct Source {
    pub at: [Fx; 3],
    /// Attached compartment; None denotes a released spill.
    pub unit: Option<usize>,
    pub intensity_milli: u32,
}
fn anchor(u: &RangeUnit, at: [u8; 3]) -> [Fx; 3] {
    let p = u
        .pose
        .world(at.map(|n| Fx::from_int(i32::from(n)).add(Fx::from_ratio(1, 2))));
    [p.x, p.y, p.z]
}
impl Combustion {
    pub fn empty(units: usize) -> Self {
        Self {
            stocks: vec![Stock::default(); units],
            tuning: Tuning::from_ron(include_str!("../../../data/range-fire.ron"))
                .expect("range fire tuning"),
            consumed: 0,
            generated_heat: 0,
            dissipated_heat: 0,
        }
    }
    /// A produced unit's stock: no fuel container. Its fuel is matter the
    /// side's store does not hold, and nothing refuels yet (ADR-675).
    pub(super) fn push_unit(&mut self) {
        self.stocks.push(Stock::default());
    }
    fn amount(&self, matter: &Matter, ml: u32) -> u64 {
        // Density units per world cell, converted from physical millilitres.
        u64::from(ml) * 1000 * u64::from(matter.source_density(self.tuning.material))
            / u64::from(WORLD_MM).pow(3)
    }
    pub fn new(units: &[RangeUnit], matter: &mut Matter) -> Self {
        let mut f = Self::empty(units.len());
        let density = matter.source_density(f.tuning.material);
        f.tuning
            .validate_density(density)
            .expect("exact fuel quantities");
        for (id, u) in units.iter().enumerate() {
            // Fixed emplacements have no mobile fuel tank. Their armour may
            // glow and burn from heat, but there is no invented fuel stock.
            if u.kind.emplacement() {
                continue;
            }
            let def = f.tuning.reservoir(u.kind);
            let [x, y, z] = fuel_cell(u.kind, def.at).map(usize::from);
            assert!(
                x < u.body.vol.nx as usize
                    && y < u.body.vol.ny as usize
                    && z < u.body.vol.nz as usize
                    && u.body.vol.get(x, y, z)
                    && u.body.part_at(x, y, z) == Part::Engine,
                "fuel anchor must be in the engine compartment"
            );
            let amount = f.amount(matter, def.fuel_ml);
            f.stocks[id].held = Some(matter.economy.ledger.create(Kind::Fuel, amount, amount));
        }
        f
    }
    pub fn remaining(&self, id: usize, matter: &Matter) -> u64 {
        self.stocks
            .get(id)
            .and_then(|s| s.held)
            .map_or(0, |c| matter.economy.ledger.get(c).held)
    }
    pub fn sources(&self, units: &[RangeUnit], matter: &Matter) -> Vec<Source> {
        let tail = self.amount(matter, self.tuning.burn_ml_per_tick)
            * u64::from(self.tuning.fade_ticks)
            * u64::from(modding::clock::STREAM_HZ)
            / u64::from(modding::clock::HZ);
        self.stocks
            .iter()
            .enumerate()
            .filter_map(|(id, s)| {
                let remaining = self.remaining(id, matter);
                if !s.burning || remaining == 0 {
                    return None;
                }
                let (at, unit) = if let Some(p) = &s.spill {
                    (p.at, None)
                } else {
                    (
                        anchor(
                            &units[id],
                            fuel_cell(units[id].kind, self.tuning.reservoir(units[id].kind).at),
                        ),
                        Some(id),
                    )
                };
                Some(Source {
                    at,
                    unit,
                    intensity_milli: (remaining * 1000 / tail.max(1)).min(1000) as u32,
                })
            })
            .collect()
    }
    pub fn save(&self, b: &mut Blob) {
        b.usize(self.stocks.len());
        for s in &self.stocks {
            b.bool(s.held.is_some());
            if let Some(id) = s.held {
                b.u32(id);
            }
            b.bool(s.burning);
            b.bool(s.spill.is_some());
            if let Some(p) = &s.spill {
                for v in p.at.into_iter().chain(p.velocity) {
                    b.fx(v);
                }
            }
        }
        b.u64(self.consumed);
        b.u64(self.generated_heat);
        b.u64(self.dissipated_heat);
    }
    pub fn load(
        &mut self,
        s: &mut Scan,
        units: &[RangeUnit],
        matter: &Matter,
        size: [u32; 3],
    ) -> Option<()> {
        if s.usize() != units.len() {
            return None;
        }
        let mut ids = BTreeSet::new();
        for (i, u) in units.iter().enumerate() {
            let held = if s.bool() {
                let id = s.u32();
                let c = matter.economy.ledger.try_get(id)?;
                if c.kind != Kind::Fuel
                    || !c.alive
                    || !ids.insert(id)
                    || c.cap != self.amount(matter, self.tuning.reservoir(u.kind).fuel_ml)
                {
                    return None;
                }
                Some(id)
            } else {
                None
            };
            let burning = s.bool();
            let spill = if s.bool() {
                let at = std::array::from_fn(|_| s.fx());
                let velocity = std::array::from_fn(|_| s.fx());
                let speed = Fx::from_ratio(self.tuning.max_speed, crate::TICKS_PER_SECOND);
                if (0..3).any(|a| {
                    at[a] < Fx::ZERO
                        || at[a] >= Fx::from_int(size[a] as i32)
                        || velocity[a].abs() > speed
                }) {
                    return None;
                }
                Some(Spill { at, velocity })
            } else {
                None
            };
            if u.kind.emplacement() && held.is_some()
                || (burning || spill.is_some()) && held.is_none()
                || burning && held.is_some_and(|id| matter.economy.ledger.get(id).held == 0)
            {
                return None;
            }
            self.stocks[i] = Stock {
                held,
                burning,
                spill,
            };
        }
        // Orphan/aliased inventory must not be accepted just because the ledger balances.
        if matter
            .economy
            .ledger
            .snapshot()
            .0
            .iter()
            .enumerate()
            .any(|(id, c)| c.kind == Kind::Fuel && c.alive && !ids.contains(&(id as u32)))
        {
            return None;
        }
        self.consumed = s.u64();
        self.generated_heat = s.u64();
        self.dissipated_heat = s.u64();
        if self.consumed > matter.economy.ledger.sunk(Sink::Combustion)
            || self.dissipated_heat > self.generated_heat
        {
            return None;
        }
        s.ok().then_some(())
    }
}
impl BlastRange {
    pub(super) fn step_unit_fire(&mut self) {
        let tuning = self.combustion.tuning.clone();
        let burn = self
            .combustion
            .amount(&self.matter, tuning.burn_ml_per_tick);
        let speed = Fx::from_ratio(tuning.max_speed, crate::TICKS_PER_SECOND);
        let gravity = Fx::from_ratio(tuning.gravity, crate::TICKS_PER_SECOND.pow(2));
        let mut emissions = Vec::new();
        // Fuel movement and receiver probes cannot change body geometry.
        // Build its bounds lazily once for this phase, including the range gun.
        let mut contact_bounds = None;
        for id in 0..self.combustion.stocks.len() {
            let mut stock = self.combustion.stocks[id].clone();
            let Some(held) = stock.held else {
                continue;
            };
            let left = self.matter.economy.ledger.get(held).held;
            if left == 0 {
                self.combustion.stocks[id].burning = false;
                continue;
            }
            let u = &self.units[id];
            let at = fuel_cell(u.kind, tuning.reservoir(u.kind).at);
            if stock.spill.is_none() {
                let c = Cell {
                    unit: id as u16,
                    part: 0,
                    at,
                };
                let breached = c.body(&self.units).is_none()
                    || u.section.as_ref().is_some_and(|s| s.holds(at));
                if breached {
                    let v = u
                        .motion
                        .drive
                        .vel
                        .div(Fx::from_int(crate::TICKS_PER_SECOND));
                    stock.spill = Some(Spill {
                        at: std::array::from_fn(|a| {
                            anchor(u, at)[a].clamp(
                                Fx::from_ratio(1, 64),
                                Fx::from_int(self.size[a] as i32).sub(Fx::from_ratio(1, 64)),
                            )
                        }),
                        velocity: [
                            fx::angle::cos(u.motion.drive.head).mul(v),
                            u.motion.hull.vy.div(Fx::from_int(crate::TICKS_PER_SECOND)),
                            fx::angle::sin(u.motion.drive.head).mul(v),
                        ]
                        .map(|v| v.clamp(Fx::ZERO.sub(speed), speed)),
                    });
                    // A penetrating break sparks/ignites the exposed contents;
                    // an unrelated loss of the unit's core does not light fuel.
                    stock.burning = true;
                } else if !stock.burning && self.thermals.hot() > 0 {
                    let radius = i32::from(tuning.ignition_radius);
                    'heat: for z in -radius..=radius {
                        for y in -radius..=radius {
                            for x in -radius..=radius {
                                let d = [x, y, z];
                                let p: [i32; 3] = std::array::from_fn(|a| d[a] + i32::from(at[a]));
                                if p.iter().any(|n| !(0..=255).contains(n)) {
                                    continue;
                                }
                                let c = Cell {
                                    at: [p[0] as u8, p[1] as u8, p[2] as u8],
                                    ..c
                                };
                                if c.body(&self.units).is_some()
                                    && temperature(self.thermals.heat(c), u.body.mm)
                                        >= tuning.ignition
                                {
                                    stock.burning = true;
                                    break 'heat;
                                }
                            }
                        }
                    }
                }
            }
            if let Some(spill) = &mut stock.spill {
                spill.velocity[1] = spill.velocity[1].sub(gravity).max(Fx::ZERO.sub(speed));
                let to = std::array::from_fn(|a| {
                    spill.at[a].add(spill.velocity[a]).clamp(
                        Fx::from_ratio(1, 64),
                        Fx::from_int(self.size[a] as i32).sub(Fx::from_ratio(1, 64)),
                    )
                });
                let bounds = contact_bounds.get_or_insert_with(|| self.heat_contact_bounds());
                if let Some(t) = self.fuel_collision(spill.at, to, bounds) {
                    // Stop just before contact; test support again next tick so
                    // a removed floor never leaves a floating, permanently resting fire.
                    let t = t.sub(Fx::from_ratio(1, 64)).max(Fx::ZERO);
                    spill.at =
                        std::array::from_fn(|a| spill.at[a].add(to[a].sub(spill.at[a]).mul(t)));
                    spill.velocity = [Fx::ZERO; 3];
                } else {
                    spill.at = to;
                }
            }
            if stock.burning {
                let spent = self.matter.economy.ledger.sink(
                    held,
                    burn * u64::from(modding::clock::stream_samples(self.tick)),
                    Sink::Combustion,
                );
                let heat =
                    (spent * u64::from(tuning.burn_ml_per_tick) * u64::from(tuning.heat_per_ml)
                        / burn) as u32;
                let origin = stock.spill.as_ref().map_or_else(|| anchor(u, at), |p| p.at);
                self.combustion.consumed += spent;
                self.combustion.generated_heat += u64::from(heat);
                emissions.push((origin, heat));
                if spent == left {
                    stock.burning = false;
                }
            }
            self.combustion.stocks[id] = stock;
        }
        // All ignitions read the old thermal field; simultaneous fires cannot
        // ignite one another in iteration order within this phase.
        if emissions.is_empty() {
            return;
        }
        let bounds = contact_bounds.get_or_insert_with(|| self.heat_contact_bounds());
        let dissipated = self.deposit_fire_batch(&emissions, tuning.reach, bounds, true);
        self.combustion.dissipated_heat += dissipated;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn range() -> BlastRange {
        let c = modding::explosion::ExplosionConfig::from_ron(include_str!(
            "../../../data/flamethrower.ron"
        ))
        .unwrap();
        let mut layout =
            modding::range::RangeLayout::from_ron(include_str!("../../../data/range-cover.ron"))
                .unwrap();
        layout.course = None;
        layout.clear_min = [64, 64];
        layout.clear_max = [600, 600];
        let mut r = BlastRange::with_layout(c, layout).unwrap();
        r.control();
        r
    }
    fn cell(r: &BlastRange, id: usize) -> Cell {
        Cell {
            unit: id as u16,
            part: 0,
            at: r.combustion.tuning.reservoir(r.units[id].kind).at,
        }
    }
    #[test]
    fn intact_and_cold_dead_bodies_do_not_burn_their_inventory() {
        let mut r = range();
        let fuel = r.combustion.remaining(0, &r.matter);
        assert_eq!(fuel, 6400);
        r.units[0].status.alive = false;
        for _ in 0..100 {
            r.step_unit_fire();
        }
        assert_eq!(r.combustion.remaining(0, &r.matter), fuel);
        assert!(r.combustion.sources(&r.units, &r.matter).is_empty());
        assert!(r.matter.economy.ledger.balanced());
    }
    #[test]
    fn density_edits_preserve_fuel_burn_heat_spill_and_restore() {
        use crate::blast::battle::commands::{Command, MaterialField};
        let mut reference = range();
        let mut edited = range();
        for r in [&mut reference, &mut edited] {
            let p = cell(r, 0);
            r.thermals.deposit(p, 120, &r.thermal_tuning);
        }
        for tick in 0..65 {
            if let Some(value) = match tick {
                0 => Some(1),
                12 => Some(modding::MAX_DENSITY),
                30 => Some(80),
                _ => None,
            } {
                edited.execute_command(Command::TuneMaterial {
                    material: 9,
                    field: MaterialField::Density,
                    value,
                });
            }
            reference.step();
            edited.step();
            assert_eq!(
                reference.combustion.remaining(0, &reference.matter),
                edited.combustion.remaining(0, &edited.matter)
            );
            assert_eq!(reference.combustion.consumed, edited.combustion.consumed);
            assert_eq!(
                reference.combustion.generated_heat,
                edited.combustion.generated_heat
            );
            let mut a = Blob::new();
            let mut b = Blob::new();
            reference.combustion.save(&mut a);
            edited.combustion.save(&mut b);
            assert_eq!(a.v, b.v);
            assert!(edited.matter.economy.ledger.balanced());
            if [1, 15, 35].contains(&tick) {
                edited = BlastRange::restore(edited.config, &edited.snapshot()).unwrap();
            }
        }
        assert!(edited.combustion.consumed > 0 && edited.combustion.generated_heat > 0);
        assert!(edited.combustion.stocks[0].spill.is_some());
        assert!(edited.thermals.melted > 0);
    }
    #[test]
    fn ignition_is_local_to_the_engine_bay_and_consumes_real_matter() {
        let mut r = range();
        let fuel = r.combustion.remaining(0, &r.matter);
        let p = cell(&r, 0);
        r.thermals
            .deposit(p, r.combustion.tuning.ignition - 1, &r.thermal_tuning);
        r.step_unit_fire();
        assert_eq!(r.combustion.remaining(0, &r.matter), fuel);
        r.thermals.deposit(p, 1, &r.thermal_tuning);
        r.step_unit_fire();
        assert_eq!(r.combustion.remaining(0, &r.matter), fuel - 4);
        assert_eq!(r.combustion.consumed, 4);
        assert_eq!(r.combustion.generated_heat, 200);
        assert_eq!(r.matter.economy.ledger.sunk(Sink::Combustion), 4);
        assert!(r.thermals.heat(p) > r.combustion.tuning.ignition);
        assert_eq!(r.combustion.remaining(1, &r.matter), fuel);
        assert!(r.matter.economy.ledger.balanced());
    }
    #[test]
    fn last_partial_burn_exhausts_fuel_and_never_restarts_from_corpse_status() {
        let mut r = range();
        let p = cell(&r, 0);
        let held = r.combustion.stocks[0].held.unwrap();
        let fuel = r.combustion.remaining(0, &r.matter);
        r.matter
            .economy
            .ledger
            .sink(held, fuel - 6, Sink::ContainerBreach);
        r.thermals.deposit(p, 120, &r.thermal_tuning);
        r.step_unit_fire();
        assert_eq!(r.combustion.remaining(0, &r.matter), 2);
        assert!(r.combustion.sources(&r.units, &r.matter)[0].intensity_milli < 1000);
        r.step_unit_fire();
        assert_eq!(r.combustion.remaining(0, &r.matter), 0);
        assert_eq!(r.combustion.consumed, 6);
        assert_eq!(r.combustion.generated_heat, 300);
        assert!(r.combustion.sources(&r.units, &r.matter).is_empty());
        r.units[0].status.alive = false;
        for _ in 0..100 {
            r.step_unit_fire();
        }
        assert_eq!(r.combustion.generated_heat, 300);
        assert_eq!(r.matter.economy.ledger.sunk(Sink::Combustion), 6);
        assert!(r.matter.economy.ledger.balanced());
    }
    #[test]
    fn burning_spill_falls_rechecks_support_and_keeps_its_container() {
        let mut r = range();
        let held = r.combustion.stocks[0].held;
        r.combustion.stocks[0].burning = true;
        r.combustion.stocks[0].spill = Some(Spill {
            at: [
                Fx::from_ratio(1001, 2),
                Fx::from_int(60),
                Fx::from_ratio(1001, 2),
            ],
            velocity: [Fx::ZERO; 3],
        });
        for _ in 0..100 {
            r.step_unit_fire();
        }
        let before = r.combustion.stocks[0].spill.as_ref().unwrap().at;
        assert!(before[1] >= Fx::from_int(16) && before[1] < Fx::from_int(17));
        assert_eq!(r.combustion.stocks[0].held, held);
        r.world.queue_dig(500, 15, 500);
        r.world.flush();
        for _ in 0..10 {
            r.step_unit_fire();
        }
        assert!(
            r.combustion.stocks[0].spill.as_ref().unwrap().at[1]
                < before[1].sub(Fx::from_ratio(1, 2))
        );
        assert!(r.matter.economy.ledger.balanced());
    }
    #[test]
    fn simultaneous_sources_do_not_ignite_other_reservoirs_in_iteration_order() {
        let mut r = range();
        let p = cell(&r, 1);
        let fuel = r.combustion.remaining(1, &r.matter);
        r.combustion.stocks[0].burning = true;
        r.combustion.stocks[0].spill = Some(Spill {
            at: anchor(&r.units[1], p.at),
            velocity: [Fx::ZERO; 3],
        });
        r.step_unit_fire();
        assert!(r.thermals.heat(p) >= 120);
        assert_eq!(r.combustion.remaining(1, &r.matter), fuel);
        r.step_unit_fire();
        assert!(r.combustion.remaining(1, &r.matter) < fuel);
    }
    #[test]
    fn compartment_breach_releases_fuel_and_burning_snapshot_replays() {
        let mut a = range();
        let p = cell(&a, 0);
        a.thermals.deposit(p, 120, &a.thermal_tuning);
        a.step();
        assert!(
            p.body(&a.units).is_none(),
            "fire must physically soften the engine cell"
        );
        a.step();
        assert!(a.combustion.stocks[0].spill.is_some());
        let mut b = BlastRange::restore(a.config, &a.snapshot()).unwrap();
        for _ in 0..65 {
            a.step();
            b.step();
            assert_eq!(a.state_hash(), b.state_hash());
        }
        assert!(a.matter.economy.ledger.balanced());
        assert!(a.thermals.melted > 0);
        assert!(a.combustion.remaining(0, &a.matter) > 0);
    }
    #[test]
    fn older_snapshots_do_not_invent_fuel_and_invalid_inventory_is_rejected() {
        let mut r = range();
        r.combustion.stocks[0].held = Some(r.matter.body_container(0));
        assert!(BlastRange::restore(r.config, &r.snapshot()).is_none());
        r.combustion = Combustion::empty(r.units.len());
        assert!(
            BlastRange::restore(r.config, &r.snapshot()).is_none(),
            "orphan fuel container"
        );
        r.matter = Matter::new(&r.units);
        let mut bytes = r.snapshot();
        let mut tail = Blob::new();
        r.save_after(&mut tail, 33);
        bytes.truncate(bytes.len() - tail.v.len());
        bytes[8..12].copy_from_slice(&33u32.to_le_bytes());
        let restored = BlastRange::restore(r.config, &bytes).unwrap();
        assert_eq!(restored.combustion.remaining(0, &restored.matter), 0);
        assert_eq!(r.state_hash(), restored.state_hash());
    }
    #[test]
    fn fuel_heat_stops_at_the_first_wall() {
        let mut r = range();
        let mut layout = r.layout.as_ref().unwrap().clone();
        layout.blocks = vec![
            modding::range::RangeBlock {
                min: [282, 16, 330],
                max: [283, 50, 342],
                material: 3,
            },
            modding::range::RangeBlock {
                min: [284, 16, 330],
                max: [285, 50, 342],
                material: 5,
            },
        ];
        r = BlastRange::with_layout(r.config, layout).unwrap();
        let at = [
            Fx::from_ratio(561, 2),
            Fx::from_ratio(61, 2),
            Fx::from_ratio(673, 2),
        ];
        assert_eq!(r.deposit_fire_heat(at, 8, 500), 500);
        assert!(r.heat.cell([282, 30, 336]).heat > 0);
        assert_eq!(r.heat.cell([284, 30, 336]).heat, 0);
    }
}
