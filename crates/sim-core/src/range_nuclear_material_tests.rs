//! Equal-size receivers isolate material response from cover and geometry.
use super::*;

struct Coupon {
    lo: [u32; 3],
    hi: [u32; 3],
    mat: u16,
}
impl Generator for Coupon {
    fn material(&self, x: u32, y: u32, z: u32) -> u16 {
        if [x, y, z]
            .iter()
            .enumerate()
            .all(|(a, &v)| self.lo[a] <= v && v < self.hi[a])
        {
            self.mat
        } else {
            0
        }
    }
    fn batches_columns(&self) -> bool {
        true
    }
    fn column(&self, x: u32, z: u32, y: u32, out: &mut [u16]) {
        for (dy, m) in out.iter_mut().enumerate() {
            *m = self.material(x, y + dy as u32, z);
        }
    }
}

fn matched_receiver(distance: i32, mat: Option<u16>) -> u32 {
    let c = ExplosionConfig::from_ron(include_str!("../../../data/nuclear-warhead.ron")).unwrap();
    let at = [256, 64, 128];
    let mut unit = RangeUnit::of_kind(RangeUnitKind::Tank, [at[0] + distance, at[1], at[2]]);
    unit.body = UnitBody::new_scaled(4, 4, 4, 4, 100);
    for z in 0..4 {
        for y in 0..4 {
            for x in 0..4 {
                unit.body.set(x, y, z, voxel::Part::Armour);
            }
        }
    }
    unit.update_pose(0);
    let lo = unit.world_pos([0; 3]).map(|v| v as u32);
    let hi = unit.world_pos([3; 3]).map(|v| v as u32 + 1);
    assert_eq!(std::array::from_fn::<_, 3, _>(|a| hi[a] - lo[a]), [4; 3]);
    let mut w = World::new(
        (distance + 768) as u32,
        128,
        256,
        65536,
        Box::new(Coupon {
            lo,
            hi,
            mat: mat.unwrap_or(0),
        }),
    );
    let mut units = if mat.is_none() { vec![unit] } else { vec![] };
    let mut dents = SurfaceWork::new();
    let mut e = Explosion::with_damage(&w, at, &c, &units, &dents);
    for _ in 0..256 {
        if !e.working() {
            break;
        }
        e.step(
            &mut w,
            c.wave_work_per_tick,
            &mut units,
            &mut dents,
            &mut Vec::new(),
            &c,
            None,
        );
        w.flush();
    }
    assert!(!e.working());
    if mat.is_none() {
        units[0].removed.len() as u32
    } else {
        let mut remaining = 0;
        for z in lo[2]..hi[2] {
            for y in lo[1]..hi[1] {
                for x in lo[0]..hi[0] {
                    remaining += u32::from(w.material(x, y, z) == mat.unwrap());
                }
            }
        }
        64 - remaining
    }
}

#[test]
fn nuclear_equal_receivers_follow_material_strength_with_modest_hull_advantage() {
    let c = ExplosionConfig::from_ron(include_str!("../../../data/nuclear-warhead.ron")).unwrap();
    let mut body = UnitBody::new_scaled(5, 1, 1, 1, 100);
    for (x, part) in [
        voxel::Part::Armour,
        voxel::Part::Core,
        voxel::Part::Engine,
        voxel::Part::Weapon,
        voxel::Part::Sensor,
    ]
    .into_iter()
    .enumerate()
    {
        body.set(x, 0, 0, part);
        let work = unit_work(&body, [x, 0, 0], &c);
        let steel = material_work(3, &c);
        assert!(work >= steel && u64::from(work) * 2 <= u64::from(steel) * 3);
    }
    for distance in [400, 800, 1000, 1200, 1600, 1800] {
        let wood = matched_receiver(distance, Some(5));
        let concrete = matched_receiver(distance, Some(2));
        let steel = matched_receiver(distance, Some(3));
        let hull = matched_receiver(distance, None);
        println!("distance={distance} wood={wood} concrete={concrete} steel={steel} hull={hull}");
        assert!(wood >= concrete && concrete >= steel && steel >= hull);
        if distance == 400 {
            assert_eq!(hull, 64);
        }
        if distance >= 1200 {
            assert_eq!(hull, 0);
        }
    }
}

#[test]
fn nuclear_paid_middle_ring_damages_forest_and_houses_without_remote_hull_breakup() {
    let (mut r, samples) = environment_with_middle(true);
    let forest = [
        sample(
            &r,
            "middle_foliage",
            true,
            [1700, 16, 500],
            [2200, 128, 750],
            4,
        ),
        sample(
            &r,
            "middle_trunks",
            true,
            [1700, 16, 500],
            [2200, 128, 750],
            5,
        ),
    ];
    let mut outer_falling_crown = false;
    for _ in 0..1100 {
        r.step();
        outer_falling_crown |= r.matter.debris.fragments.iter().any(|piece| {
            let dx = piece.position.x.to_int_trunc() - 1024;
            let dz = piece.position.z.to_int_trunc() - 1000;
            piece.material == 4
                && piece.side == 2
                && !piece.resting
                && (700 * 700..=1400 * 1400).contains(&(dx * dx + dz * dz))
        });
    }
    assert!(
        outer_falling_crown,
        "no falling crown pieces in the 70–140m belt"
    );
    println!("nuclear crown release={:?}", r.matter.admission.felling);
    assert!(
        r.matter.admission.felling[3] > 1000,
        "forest did not crumble as debris"
    );
    for s in samples
        .into_iter()
        .filter(|s| s.near && (1800..1900).contains(&s.cells[0].0[0]))
        .chain(forest)
    {
        let left = s
            .cells
            .iter()
            .filter(|&&(p, m)| r.world.material(p[0], p[1], p[2]) == m)
            .count();
        println!("{} initial={} remaining={left}", s.name, s.cells.len());
        match s.name {
            "wood" => assert!(left * 2 < s.cells.len()),
            "concrete" => assert!(left * 4 < s.cells.len() * 3),
            "middle_foliage" => assert!(left * 2 < s.cells.len()),
            "middle_trunks" => assert!(left * 3 < s.cells.len() * 2),
            _ => {}
        }
    }
    assert_eq!(r.battle.actors[0].fired, 1);
    assert!(r.units[0].status.alive && r.units[0].revision == 0);
    assert!(r.matter.economy.ledger.balanced());
}
