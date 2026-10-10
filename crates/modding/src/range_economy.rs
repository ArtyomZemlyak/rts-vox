//! Dig economy of the range, by unit kind (plan 245, slice 0; ADR-672).
//!
//! Every quantity is in world units: a tank holds matter units (one cell
//! of a material carries its density, `range-materials.ron`), radii and
//! reach are world cells. The one count of voxels is the dig sub-pool:
//! it protects the tick, not the economy (ADR-045).
use crate::range::RangeUnitKind;
use serde::{Deserialize, Serialize};

/// Largest dig radius, world units. A digger scans its ball every tick,
/// so the ball is bounded like any other per-tick work (ADR-114).
pub const MAX_DIG_RADIUS: u32 = 32;
/// Largest reach between a digger and its face, world units.
pub const MAX_DIG_REACH: u32 = 1024;
/// Largest tank, matter units; sums of tanks stay far from overflow.
pub const MAX_TANK: u64 = 1 << 40;
/// Largest dig sub-pool, edits in flight.
pub const MAX_DIG_POOL: u32 = 1 << 20;

/// What one kind does in the economy. A kind with an empty tank does not
/// dig, and then carries no dig parameters at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct KindEconomy {
    pub kind: RangeUnitKind,
    /// Matter the digger's tank holds, matter units.
    pub tank: u64,
    /// Radius of the ball dug around the face, world units.
    pub dig_radius: u32,
    /// Horizontal distance from the digger to its face within which it
    /// digs, world units.
    pub dig_reach: u32,
    /// Dig edits one digger may queue in a tick.
    pub dig_edits_per_tick: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct RangeEconomy {
    /// Dig edits of all diggers together that may wait in the world's edit
    /// queue: the dig sub-pool inside the shared edit ceiling, so a dig
    /// never starves combat edits.
    pub dig_edits_in_flight: u32,
    /// At most one entry per kind; a kind without one does not dig.
    pub kinds: Vec<KindEconomy>,
}

impl RangeEconomy {
    pub fn from_ron(text: &str) -> Result<Self, String> {
        let e: Self = ron::from_str(text).map_err(|e| e.to_string())?;
        e.validate()?;
        Ok(e)
    }
    /// The stock table, `data/range-economy.ron`.
    pub fn stock() -> Self {
        Self::from_ron(include_str!("../../../data/range-economy.ron")).expect("stock economy")
    }
    pub fn validate(&self) -> Result<(), String> {
        if !(1..=MAX_DIG_POOL).contains(&self.dig_edits_in_flight)
            || self.kinds.len() > RangeUnitKind::COUNT
        {
            return Err("range economy exceeds supported bounds".into());
        }
        for (i, k) in self.kinds.iter().enumerate() {
            if self.kinds[..i].iter().any(|o| o.kind == k.kind) {
                return Err("range economy lists a kind twice".into());
            }
            let digs = [k.dig_radius, k.dig_reach, k.dig_edits_per_tick];
            let valid = if k.tank == 0 {
                digs == [0; 3]
            } else {
                k.tank <= MAX_TANK
                    && (1..=MAX_DIG_RADIUS).contains(&k.dig_radius)
                    && (1..=MAX_DIG_REACH).contains(&k.dig_reach)
                    && (1..=self.dig_edits_in_flight).contains(&k.dig_edits_per_tick)
            };
            if !valid {
                return Err("range economy kind exceeds supported bounds".into());
            }
        }
        Ok(())
    }
    /// The kind's entry, if it has one.
    pub fn kind(&self, kind: RangeUnitKind) -> Option<&KindEconomy> {
        self.kinds.iter().find(|k| k.kind == kind)
    }
    pub fn kind_mut(&mut self, kind: RangeUnitKind) -> Option<&mut KindEconomy> {
        self.kinds.iter_mut().find(|k| k.kind == kind)
    }
    /// Whether this kind digs: its tank holds something.
    pub fn digs(&self, kind: RangeUnitKind) -> bool {
        self.kind(kind).is_some_and(|k| k.tank > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stock_economy_lists_every_kind_once_and_none_digs() {
        let e = RangeEconomy::stock();
        for kind in RangeUnitKind::ALL {
            assert!(e.kind(kind).is_some(), "{kind:?}");
            assert!(!e.digs(kind), "the owner has not chosen the digger");
        }
    }

    #[test]
    fn the_loader_refuses_broken_economies() {
        let mut e = RangeEconomy::stock();
        let tank = e.kind_mut(RangeUnitKind::Tank).unwrap();
        tank.tank = 12_000;
        tank.dig_radius = 3;
        tank.dig_reach = 40;
        tank.dig_edits_per_tick = 16;
        e.validate().unwrap();
        let text = ron::ser::to_string(&e).unwrap();
        assert_eq!(RangeEconomy::from_ron(&text).unwrap(), e);
        let broken: [fn(&mut RangeEconomy); 7] = [
            |e| e.dig_edits_in_flight = 0,
            |e| e.kinds[0].dig_radius = 0,
            |e| e.kinds[0].dig_radius = MAX_DIG_RADIUS + 1,
            |e| e.kinds[0].dig_edits_per_tick = e.dig_edits_in_flight + 1,
            |e| e.kinds[0].tank = MAX_TANK + 1,
            // An empty tank carries no inert dig parameters.
            |e| e.kinds[1].dig_reach = 5,
            |e| e.kinds[1].kind = e.kinds[0].kind,
        ];
        for f in broken {
            let mut b = e.clone();
            f(&mut b);
            assert!(b.validate().is_err(), "{b:?}");
        }
    }
}
