//! Contention between real, resumable blast contacts and a finite flying pool.
use super::*;
use voxel::unit::UnitBody;

fn contested_waves() -> BlastRange {
    let mut layout = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
    layout.battle = true;
    layout.blocks.clear();
    layout.shields.clear();
    layout.course = None;
    layout.units = (0..3)
        .map(|i| modding::range::RangeUnitDef {
            weapon: None,
            kind: RangeUnitKind::Tank,
            at: [170 + i * 100, 80, 336],
            allied: i == 0,
        })
        .collect();
    let config = ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    let mut r = BlastRange::with_layout(config, layout).unwrap();
    for u in &mut r.units {
        u.body = UnitBody::new_scaled(10, 1, 1, 0, 100);
        for x in 0..10 {
            u.body.set(x, 0, 0, voxel::Part::Armour);
        }
        u.update_pose(0);
    }
    r.matter = matter::Matter::new(&r.units);
    r.matter.tuning.max_flying = 1;
    let c = &mut r.battle.shell_config;
    c.energy = 1;
    c.cell_units = 1;
    c.half_cells = 16;
    c.unit_ablation_resistance = 1;
    c.unit_role_resistance_milli = [1000; 5];
    c.material_energy_j = 1_000_000;
    for u in &r.units {
        let mut wave = Explosion::with_damage(&r.world, u.at, c, &r.units, &r.surface_work);
        // The propagation is complete; only the ordered, physical body hits
        // remain. Every wave still needs several slots, across several ticks.
        wave.wave.advance_contacts(usize::MAX);
        for x in 0..10 {
            let p = u.pose.world([
                Fx::from_ratio(2 * x + 1, 2),
                Fx::from_ratio(1, 2),
                Fx::from_ratio(1, 2),
            ]);
            let p = [p.x, p.y, p.z].map(|v| (v.raw() >> 32) as i32);
            wave.pending.push_back(phys::blast::Hit {
                cell: std::array::from_fn(|a| p[a] - wave.origin[a]),
                pressure: 100_000,
                broken: true,
            });
        }
        r.battle.waves.push(Burst {
            weapon: Weapon::Blast,
            wave,
            energy: u64::from(c.material_energy_j) * 1000,
        });
    }
    r
}

fn apply_waves(r: &mut BlastRange) {
    let mut b = std::mem::replace(&mut r.battle, Battle::new(&r.units, 1));
    r.battle_waves(&mut b, &mut |_| {});
    r.battle = b;
}

fn settle_admitted(r: &mut BlastRange) {
    for f in &mut r.matter.debris.fragments {
        f.resting = true;
        f.velocity = phys::V3::ZERO;
    }
}

#[test]
fn wave_admission_gives_each_waiting_burst_a_turn_without_exceeding_pool() {
    let mut r = contested_waves();
    for tick in 0..3 {
        r.tick = tick;
        settle_admitted(&mut r);
        apply_waves(&mut r);
        assert!(r.matter.debris.fragment_pools(&r.matter.tuning).flying <= 1);
        assert!(r.matter.economy.ledger.balanced());
        assert!(
            !r.units[tick as usize].removed.is_empty(),
            "wave {tick} starved behind an earlier burst"
        );
    }
    assert_eq!(r.battle.waves.len(), 3, "older waves must still be pending");
    assert!(
        r.matter.admission.body[2] > 0,
        "fixture did not saturate admission"
    );
    for tick in 3..24 {
        r.tick = tick;
        settle_admitted(&mut r);
        apply_waves(&mut r);
        if r.battle.waves.is_empty() {
            break;
        }
    }
    assert!(
        r.battle.waves.is_empty(),
        "a deferred contact was lost or stuck"
    );
    assert!(r.units.iter().all(|u| u.removed.len() == 10));
    assert_eq!(
        r.matter
            .debris
            .fragments
            .iter()
            .map(|f| f.count)
            .sum::<u32>(),
        30
    );
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn wave_admission_keeps_blocked_contacts_and_resumes_the_saved_turn() {
    let mut a = contested_waves();
    let mut b = contested_waves();
    for r in [&mut a, &mut b] {
        r.matter.tuning.max_flying = 0;
        apply_waves(r);
        assert!(r.units.iter().all(|u| u.removed.is_empty()));
        assert!(r.matter.debris.fragments.is_empty());
        assert!(r.battle.waves.iter().all(|w| w.wave.pending.len() == 10));
        r.matter.tuning.max_flying = 1;
    }
    for tick in 0..24 {
        for r in [&mut a, &mut b] {
            r.tick = tick;
            settle_admitted(r);
            apply_waves(r);
        }
        if tick == 1 {
            let mut bytes = Blob::new();
            b.battle.save(&mut bytes);
            b.battle.save_wave_contacts(&mut bytes);
            let mut scan = Scan::new(&bytes.v);
            let mut restored = Battle::new(&b.units, 1);
            restored.shell_config = b.battle.shell_config.clone();
            let allied: Vec<_> = (0..b.units.len()).map(|id| b.allied(id)).collect();
            restored
                .load(
                    &mut scan,
                    &b.units,
                    b.size,
                    b.turret_limits,
                    (48, b.tick, &allied),
                )
                .unwrap();
            restored.load_wave_contacts(&mut scan, &b.units).unwrap();
            assert!(scan.ok());
            assert_eq!(scan.left(), 0);
            b.battle = restored;
        }
        // Includes matter, energy, canonical wave order and exact contact
        // cursors: loading must not repeat paid damage or change who gets room.
        assert_eq!(a.state_hash(), b.state_hash(), "tick {tick}");
    }
    assert!(a.battle.waves.is_empty());
    assert_eq!(
        a.matter
            .debris
            .fragments
            .iter()
            .map(|f| f.count)
            .sum::<u32>(),
        30
    );
    assert!(a.matter.economy.ledger.balanced());
}
