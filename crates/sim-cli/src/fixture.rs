//! Shared setup of the visual-QA fixtures on the range (ADR-662, ADR-671).
//!
//! A fixture lays out a range from a preset in `data/`, drives it with the
//! same orders a player gives, applies a diagnostic dose through a normal
//! handler of the simulation and writes `BlastRange::snapshot()`. The
//! window opens the file with `blast --load`. The check then runs on the
//! restored copy, so the save itself is what is verified.
// Each fixture binary includes this module and uses part of it.
#![allow(dead_code)]
use modding::explosion::ExplosionConfig;
use modding::range::RangeLayout;
use sim_core::blast::{thermal, BlastRange};

/// The weapon preset of `blast` without `--weapon`: the window restores
/// the save with the same configuration.
pub fn config() -> ExplosionConfig {
    ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).expect("explosion.ron")
}

/// A prepared roster under free control: orders move it, no AI, no fire.
pub fn range(layout: &str) -> BlastRange {
    let layout = RangeLayout::from_ron(layout).expect("fixture layout");
    let mut r = BlastRange::with_layout(config(), layout).expect("valid fixture layout");
    r.start_free_control().expect("fixture roster");
    r
}

/// Output path: the first argument or the fixture's default.
pub fn path(default: &str) -> String {
    std::env::args().nth(1).unwrap_or_else(|| default.into())
}

pub fn order_move(r: &mut BlastRange, unit: usize, to: [i32; 2]) {
    assert!(
        r.battle_order(unit, sim_core::blast::battle::Order::Move, Some(to)),
        "unit {unit} refused a move to {to:?}"
    );
}

/// Write the snapshot and return it restored. A save that does not read
/// back to the same bytes is refused here, not in the window.
pub fn save(path: &str, r: &BlastRange) -> BlastRange {
    let bytes = r.snapshot();
    if let Some(parent) = std::path::Path::new(path).parent() {
        std::fs::create_dir_all(parent).expect("fixture directory");
    }
    std::fs::write(path, &bytes).expect("fixture file");
    let read = std::fs::read(path).expect("fixture file");
    let restored = BlastRange::restore(config(), &read).expect("fixture snapshot restores");
    assert_eq!(restored.snapshot(), bytes, "snapshot round trip");
    assert_eq!(restored.state_hash(), r.state_hash(), "restored hash");
    restored
}

/// A diagnostic heat dose: each listed cell of `part` (0 is the root,
/// otherwise segment index + 1) receives twice its role's softening
/// temperature. The range's own thermal step melts the cells into debris
/// on the following ticks; neighbours receive a conducted share far below
/// softening. This is not the dose of a stock weapon.
pub fn melt(r: &mut BlastRange, unit: usize, part: u8, cells: &[[u8; 3]]) {
    let u = &r.units[unit];
    let body = if part == 0 {
        &u.body
    } else {
        &u.body.segments[usize::from(part - 1)].body
    };
    for &at in cells {
        let [x, y, z] = at.map(usize::from);
        assert!(body.vol.get(x, y, z), "melt target {at:?} is not solid");
        let role = body.part_at(x, y, z) as usize;
        let target = 2 * r.thermal_tuning.softening[role];
        // One heat unit per 100 mm cube (WORLD_MM): a larger cell needs more.
        let heat = (u64::from(target) * u64::from(body.mm).pow(3) / 1_000_000) as u32;
        assert!(thermal::temperature(heat, body.mm) >= target);
        r.thermals.deposit(
            thermal::Cell {
                unit: unit as u16,
                part,
                at,
            },
            heat,
            &r.thermal_tuning,
        );
    }
    assert_eq!(r.thermals.rejected_heat, 0, "thermal field full");
}

/// Ground removed from under the units: queued digs that the next tick's
/// edit flush applies, the matter of every cell sunk as annihilation.
/// Navigation, collapse and contacts see the hole in that same tick.
pub fn dig(r: &mut BlastRange, cells: impl IntoIterator<Item = [u32; 3]>) -> usize {
    let mut removed = Vec::new();
    for [x, y, z] in cells {
        let mat = r.world.material(x, y, z);
        if mat != 0 {
            r.world.queue_dig(x, y, z);
            removed.push(([x, y, z], mat));
        }
    }
    r.matter.annihilate_cells(&removed);
    removed.len()
}

pub fn balanced(r: &BlastRange) -> bool {
    r.matter.economy.ledger.balanced()
}

/// Solid cells of a unit's root that satisfy `keep`, in volume order.
pub fn root_cells(r: &BlastRange, unit: usize, keep: impl Fn([u8; 3]) -> bool) -> Vec<[u8; 3]> {
    let b = &r.units[unit].body;
    let mut out = Vec::new();
    for z in 0..b.vol.nz {
        for y in 0..b.vol.ny {
            for x in 0..b.vol.nx {
                if b.vol.get(usize::from(x), usize::from(y), usize::from(z)) && keep([x, y, z]) {
                    out.push([x, y, z]);
                }
            }
        }
    }
    out
}

/// Whole pieces of a unit's root handed to debris: (pieces, cells).
pub fn root_sections(r: &BlastRange, unit: usize) -> (usize, u32) {
    r.matter
        .debris
        .fragments
        .iter()
        .filter(|f| {
            f.chunk
                .as_ref()
                .is_some_and(|c| usize::from(c.template) == unit && c.segment.is_none())
        })
        .fold((0, 0), |(n, cells), f| (n + 1, cells + f.count))
}
