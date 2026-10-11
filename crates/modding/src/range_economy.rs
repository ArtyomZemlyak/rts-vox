//! Dig economy of the range, by unit kind (plan 245, slices 0, 3 and 6;
//! ADR-672, ADR-673, ADR-674).
//!
//! Every quantity is in world units: a tank and a store hold matter units
//! (one cell of a material carries its density, `range-materials.ron`),
//! radii and reach are world cells. The one count of voxels is the dig
//! sub-pool: it protects the tick, not the economy (ADR-045).
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
/// Largest store of a dock and largest base store, matter units: sixty-four
/// docks and the base together stay far from overflow.
pub const MAX_STORE: u64 = 1 << 48;
/// Largest denominator of the refining yield.
pub const MAX_REFINE_DEN: u64 = 1 << 20;
/// Largest number of dock searches in a tick.
pub const MAX_DOCK_SEARCHES: u32 = 1024;
/// Largest number of reclaim searches in a tick.
pub const MAX_RECLAIM_SEARCHES: u32 = 1024;

fn is_zero(v: &u64) -> bool {
    *v == 0
}

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
    /// Capacity a living unit of this kind adds to its side's store,
    /// matter units (ADR-673). A kind with a store is a dock: haulers of
    /// its side unload there. Absent in the RON of a slice-1 layout.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub store: u64,
}

/// The sides' stores and unloading (plan 245, slice 3; ADR-673). Without
/// it a full tank stops digging and nobody hauls.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct RangeBank {
    /// Capacity of each side's store with no dock standing, matter units.
    pub base_store: u64,
    /// Refining yield at unloading, `refine_num / refine_den`, below one:
    /// the rest goes to the RefiningLoss sink.
    pub refine_num: u64,
    pub refine_den: u64,
    /// Horizontal distance from a hauler to its dock within which it
    /// unloads, world units.
    pub unload_reach: u32,
    /// Dock searches of all haulers together in one tick; the rest wait
    /// for the next tick (ADR-114).
    pub dock_searches_per_tick: u32,
}

/// Reclaim at the face and the wreck of a dead digger's tank (plan 245,
/// slice 6; ADR-674). Without it nothing is reclaimed and a dead digger's
/// cargo stays whole in its tank.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct RangeReclaim {
    /// Distance from a digger's face to the centre of a resting piece, or
    /// to a dead digger's hull, within which the digger takes it, world
    /// units.
    pub reach: u32,
    /// Reclaim searches of all diggers together in one tick; the rest wait
    /// for the next tick (ADR-114).
    pub searches_per_tick: u32,
    /// Share of a dead digger's cargo lost to ContainerBreach,
    /// `breach_num / breach_den`; the rest lies with the hull.
    pub breach_num: u64,
    pub breach_den: u64,
    /// Matter one dig edit takes from a dead digger's tank, which has no
    /// cells to count, matter units.
    pub cargo_per_edit: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct RangeEconomy {
    /// Dig edits of all diggers together that may wait in the world's edit
    /// queue: the dig sub-pool inside the shared edit ceiling, so a dig
    /// never starves combat edits.
    pub dig_edits_in_flight: u32,
    /// At most one entry per kind; a kind without one does not dig.
    pub kinds: Vec<KindEconomy>,
    /// Stores and hauling; absent in the RON of a slice-1 layout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bank: Option<RangeBank>,
    /// Reclaim; absent in the RON of a slice-1 or slice-3 layout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reclaim: Option<RangeReclaim>,
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
            if !valid || k.store > MAX_STORE || (k.store > 0 && self.bank.is_none()) {
                return Err("range economy kind exceeds supported bounds".into());
            }
        }
        if let Some(b) = &self.bank {
            if b.base_store > MAX_STORE
                || !(1..=MAX_REFINE_DEN).contains(&b.refine_den)
                || !(1..b.refine_den).contains(&b.refine_num)
                || !(1..=MAX_DIG_REACH).contains(&b.unload_reach)
                || !(1..=MAX_DOCK_SEARCHES).contains(&b.dock_searches_per_tick)
            {
                return Err("range economy bank exceeds supported bounds".into());
            }
        }
        if let Some(r) = &self.reclaim {
            if !(1..=MAX_DIG_REACH).contains(&r.reach)
                || !(1..=MAX_RECLAIM_SEARCHES).contains(&r.searches_per_tick)
                || !(1..=MAX_REFINE_DEN).contains(&r.breach_den)
                || r.breach_num > r.breach_den
                || !(1..=u64::from(crate::MAX_DENSITY)).contains(&r.cargo_per_edit)
            {
                return Err("range economy reclaim exceeds supported bounds".into());
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
    /// Store a living unit of this kind adds to its side; a dock has one.
    pub fn store(&self, kind: RangeUnitKind) -> u64 {
        self.kind(kind).map_or(0, |k| k.store)
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
        // The base is the stock dock (ADR-673).
        assert!(e.bank.is_some());
        assert!(e.store(RangeUnitKind::Base) > 0);
        assert_eq!(e.store(RangeUnitKind::Tank), 0);
        // Reclaim is on (ADR-674).
        assert!(e.reclaim.is_some());
    }

    #[test]
    fn a_slice_one_economy_reads_and_writes_without_a_bank() {
        let mut e = RangeEconomy::stock();
        e.bank = None;
        e.reclaim = None;
        e.kinds.iter_mut().for_each(|k| k.store = 0);
        let text = ron::ser::to_string(&e).unwrap();
        assert!(
            !text.contains("store") && !text.contains("bank") && !text.contains("reclaim"),
            "{text}"
        );
        assert_eq!(RangeEconomy::from_ron(&text).unwrap(), e);
    }

    #[test]
    fn a_slice_three_economy_reads_and_writes_without_reclaim() {
        let mut e = RangeEconomy::stock();
        e.reclaim = None;
        let text = ron::ser::to_string(&e).unwrap();
        assert!(text.contains("bank") && !text.contains("reclaim"), "{text}");
        assert_eq!(RangeEconomy::from_ron(&text).unwrap(), e);
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
        let broken: [fn(&mut RangeEconomy); 19] = [
            |e| e.dig_edits_in_flight = 0,
            |e| e.kinds[0].dig_radius = 0,
            |e| e.kinds[0].dig_radius = MAX_DIG_RADIUS + 1,
            |e| e.kinds[0].dig_edits_per_tick = e.dig_edits_in_flight + 1,
            |e| e.kinds[0].tank = MAX_TANK + 1,
            // An empty tank carries no inert dig parameters.
            |e| e.kinds[1].dig_reach = 5,
            |e| e.kinds[1].kind = e.kinds[0].kind,
            // A dock needs the bank its store raises.
            |e| e.bank = None,
            |e| e.kinds[0].store = MAX_STORE + 1,
            |e| e.bank.as_mut().unwrap().refine_num = 0,
            // The refining yield is below one: the sink is on (07-economy).
            |e| {
                let b = e.bank.as_mut().unwrap();
                b.refine_num = b.refine_den;
            },
            |e| e.bank.as_mut().unwrap().unload_reach = 0,
            |e| e.bank.as_mut().unwrap().dock_searches_per_tick = 0,
            |e| e.reclaim.as_mut().unwrap().reach = 0,
            |e| e.reclaim.as_mut().unwrap().reach = MAX_DIG_REACH + 1,
            |e| e.reclaim.as_mut().unwrap().searches_per_tick = 0,
            |e| e.reclaim.as_mut().unwrap().breach_den = 0,
            // The breach loses no more than the cargo.
            |e| {
                let r = e.reclaim.as_mut().unwrap();
                r.breach_num = r.breach_den + 1;
            },
            |e| e.reclaim.as_mut().unwrap().cargo_per_edit = 0,
        ];
        for f in broken {
            let mut b = e.clone();
            f(&mut b);
            assert!(b.validate().is_err(), "{b:?}");
        }
    }
}
