//! Real terrain, hollow buildings and procedural woodland, with paid damage.
use super::*;
use modding::range::{RangeBlock, RangeUnitDef};

#[path = "range_nuclear_material_tests.rs"]
mod materials;

struct Air;
impl Generator for Air {
    fn material(&self, _: u32, _: u32, _: u32) -> u16 {
        0
    }
    fn column(&self, _: u32, _: u32, _: u32, out: &mut [u16]) {
        out.fill(0);
    }
}

#[test]
fn nuclear_environment_shatters_near_steel_without_relying_on_structural_collapse() {
    let c = ExplosionConfig::from_ron(include_str!("../../../data/nuclear-warhead.ron")).unwrap();
    let mut w = World::new(768, 128, 128, 65536, Box::new(Air));
    for z in 60..68 {
        for y in 40..48 {
            for x in 400..408 {
                w.queue_edit(voxel::Edit::put(x, y, z, 3));
            }
        }
    }
    while w.queued() > 0 {
        w.flush();
    }
    let mut dents = SurfaceWork::new();
    let mut e = Explosion::with_damage(&w, [128, 40, 64], &c, &[], &dents);
    for _ in 0..128 {
        if !e.working() {
            break;
        }
        e.step(
            &mut w,
            c.wave_work_per_tick,
            &mut [],
            &mut dents,
            &mut Vec::new(),
            &c,
            None,
        );
        while w.queued() > 0 {
            w.flush();
        }
    }
    assert!(!e.working());
    for z in 60..68 {
        for y in 40..48 {
            for x in 400..408 {
                assert_eq!(w.material(x, y, z), 0, "steel remained at {x}/{y}/{z}");
            }
        }
    }
}

struct Sample {
    name: &'static str,
    near: bool,
    cells: Vec<([u32; 3], u16)>,
}

fn sample(
    r: &BlastRange,
    name: &'static str,
    near: bool,
    min: [u32; 3],
    max: [u32; 3],
    mat: u16,
) -> Sample {
    let mut cells = Vec::new();
    let mut column = vec![0; (max[1] - min[1]) as usize];
    for z in min[2]..max[2] {
        for x in min[0]..max[0] {
            r.world.column(x, z, min[1], &mut column);
            for (i, &m) in column.iter().enumerate() {
                if m == mat {
                    cells.push(([x, min[1] + i as u32, z], m));
                }
            }
        }
    }
    assert!(!cells.is_empty(), "empty fixture: {name}");
    Sample { name, near, cells }
}

fn environment() -> (BlastRange, Vec<Sample>) {
    environment_with_middle(false)
}

fn environment_with_middle(middle: bool) -> (BlastRange, Vec<Sample>) {
    let c = ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    let mut l = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
    l.battle = true;
    l.idle_enemies = true;
    l.size = [4608, 192, 2048];
    l.clear_min = [64, 800];
    l.clear_max = [4500, 1200];
    l.course = None;
    l.shields.clear();
    l.blocks.clear();
    l.units = [
        (RangeUnitKind::NuclearSilo, [3900, 16, 1000], true),
        (RangeUnitKind::Base, [1200, 16, 900], false),
    ]
    .into_iter()
    .map(|(kind, at, allied)| RangeUnitDef {
        kind,
        at,
        allied,
        weapon: None,
    })
    .collect();
    let mut boxes = Vec::new();
    let sites = if middle {
        vec![(true, 1400), (true, 1800), (false, 2820)]
    } else {
        vec![(true, 1400), (false, 2820)]
    };
    for (near, x) in sites {
        for (name, mat, z) in [("wood", 5, 820), ("concrete", 2, 1040), ("steel", 3, 1130)] {
            let min = [x, 16, z];
            let max = [x + 60, 56, z + 48];
            l.blocks.extend([
                RangeBlock {
                    min,
                    max,
                    material: mat,
                },
                RangeBlock {
                    min: [x + 4, 16, z + 4],
                    max: [x + 56, 52, z + 44],
                    material: 0,
                },
            ]);
            boxes.push((
                name,
                near,
                min.map(|v| v as u32),
                max.map(|v| v as u32),
                mat,
            ));
        }
    }
    let mut r = BlastRange::with_layout(c, l).unwrap();
    r.set_physics_workers(4);
    r.set_navigation_workers(4);
    let mut samples: Vec<_> = boxes
        .into_iter()
        .map(|(name, near, min, max, mat)| sample(&r, name, near, min, max, mat))
        .collect();
    // Both stems and crowns come from RangeTerrain; no decorative tree props.
    for (near, min, max) in [
        (true, [850, 16, 500], [1350, 128, 750]),
        (false, [2800, 16, 500], [3000, 128, 750]),
    ] {
        samples.push(sample(&r, "foliage", near, min, max, 4));
        samples.push(sample(&r, "trunks", near, min, max, 5));
    }
    r.start_battle().unwrap();
    assert!(r.battle_order(0, Order::Ground([1024, 15, 1000]), None));
    (r, samples)
}

#[test]
fn nuclear_environment_destroys_near_buildings_and_forest_but_preserves_distant_controls() {
    let (mut r, samples) = environment();
    let mut impact = None;
    let mut impact_tick = None;
    let mut finished_tick = None;
    for _ in 0..1100 {
        r.step();
        if let Some(i) = r
            .battle
            .impacts
            .iter()
            .find(|i| i.weapon == Weapon::Nuclear)
        {
            impact = Some(i.at);
            impact_tick = Some(r.tick);
        }
        if impact_tick.is_some()
            && finished_tick.is_none()
            && r.battle.waves.is_empty()
            && r.world.queued() == 0
        {
            finished_tick = Some(r.tick);
        }
    }
    // A building on the incoming path would turn this into a different blast.
    assert_eq!(impact, Some([1024, 15, 1000]));
    assert!(
        finished_tick.is_some_and(|tick| tick - impact_tick.unwrap() <= 10 * modding::clock::HZ),
        "destruction exceeded ten seconds: impact={impact_tick:?} finished={finished_tick:?}"
    );
    assert_eq!(r.battle.actors[0].fired, 1);
    for s in samples {
        let left = s
            .cells
            .iter()
            .filter(|&&(p, m)| r.world.material(p[0], p[1], p[2]) == m)
            .count();
        println!(
            "{} near={} initial={} remaining={left}",
            s.name,
            s.near,
            s.cells.len()
        );
        if s.near {
            match s.name {
                "wood" | "concrete" => {
                    assert_eq!(left, 0, "near {} remained standing", s.name)
                }
                "steel" => assert!(
                    left * 4 < s.cells.len(),
                    "near steel building remained intact"
                ),
                "foliage" => assert!(
                    left * 3 < s.cells.len(),
                    "outer grove crown damage too small"
                ),
                "trunks" => assert!(
                    left * 3 < s.cells.len(),
                    "outer grove trunk damage too small"
                ),
                _ => unreachable!(),
            }
        } else {
            assert_eq!(left, s.cells.len(), "distant {} damaged", s.name);
        }
    }
    assert!(!r.units[1].status.alive, "near base survived");
    assert!(r.units[0].status.alive && r.units[0].revision == 0);
    assert!(
        r.removed_materials[2] > 0
            && r.removed_materials[3] > 0
            && r.removed_materials[4] > 0
            && r.removed_materials[5] > 0
    );
    assert!(r.matter.economy.ledger.balanced());
}
