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
            side: None,
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
            b.battle.save_contact_resumes(&mut bytes);
            let mut scan = Scan::new(&bytes.v);
            let mut restored = Battle::new(&b.units, 1);
            restored.shell_config = b.battle.shell_config;
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
            restored.load_contact_resumes(&mut scan, &b.units).unwrap();
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

// Pre-cache reference: independently recount before every atomic wave.
impl BlastRange {
    fn scalar_waves(&mut self, b: &mut Battle, completed: &mut impl FnMut(&'static str)) {
        let n = b.waves.len().max(1);
        // A growing old crater must not consume every newly freed fragment
        // slot before a newer hit reaches its hull. Rotate service, not the
        // canonical wave array: snapshots and stable wave identity keep their
        // order, and the saved tick determines the next turn after loading.
        let first = self.tick as usize % n;
        for offset in 0..b.waves.len() {
            let w = &mut b.waves[(first + offset) % n];
            let config = match w.weapon {
                Weapon::Energy => &b.energy_config,
                Weapon::Annihilation => &b.anni_config,
                Weapon::Canister => &b.canister_config,
                _ => &b.shell_config,
            };
            if w.wave.capturing() {
                w.wave.capture(
                    &self.world,
                    config,
                    &self.units,
                    &self.surface_work,
                    wave_share(config.capture_cells_per_tick, n, offset),
                );
                completed("battle_capture");
            }
            if !w.wave.capturing() {
                let old = self.matter.remaining_mj;
                self.matter.remaining_mj = w.energy;
                self.matter.fragment_size(config.fragment_box);
                let before: Vec<_> = self
                    .units
                    .iter()
                    .map(|u| (u.removed.len(), u.seg_removed.len()))
                    .collect();
                w.wave.step_with_room::<true>(
                    &mut self.world,
                    wave_share(config.wave_work_per_tick, n, offset),
                    &mut self.units,
                    &mut self.surface_work,
                    &mut self.surface_changed,
                    config,
                    Some(&mut self.matter),
                    None,
                );
                if config.material_energy_j == 0 {
                    for (id, (u, (root, seg))) in self.units.iter().zip(before).enumerate() {
                        self.matter.annihilate_body(
                            id,
                            u.removed.len() - root + u.seg_removed.len() - seg,
                        );
                    }
                }
                completed("battle_wave_step");
                w.energy = self.matter.remaining_mj;
                self.matter.remaining_mj = old;
                for u in &mut self.units {
                    if u.absorbed > 0 && u.status.alive {
                        u.motion.impulse(
                            &u.body,
                            w.wave.at,
                            u64::from(u.absorbed) * u64::from(config.hull_impulse_milli) / 1000,
                        );
                    }
                    u.absorbed = 0;
                }
            }
        }
        b.waves.retain(|w| w.wave.working());
    }
}

fn apply_scalar_waves(r: &mut BlastRange) {
    let mut b = std::mem::replace(&mut r.battle, Battle::new(&r.units, 1));
    r.scalar_waves(&mut b, &mut |_| {});
    r.battle = b;
}

fn saved_contacts(r: &BlastRange) -> Vec<u8> {
    let mut b = Blob::new();
    r.world.save(&mut b);
    r.matter.save(&mut b);
    r.battle.save(&mut b);
    r.battle.save_wave_contacts(&mut b);
    r.battle.save_contact_resumes(&mut b);
    b.v
}

#[test]
fn wave_phase_capacity_matches_scalar_at_every_pool_boundary() {
    for cap in [0, 1, 3, 64] {
        let mut a = contested_waves();
        let mut b = contested_waves();
        for r in [&mut a, &mut b] {
            r.matter.tuning.max_flying = cap;
            r.matter.tuning.max_fragments = 64;
        }
        for tick in 0..24 {
            for r in [&mut a, &mut b] {
                r.tick = tick;
                // Capacity changes between phases, so a cross-tick cache would
                // keep waiting after these pieces settle.
                settle_admitted(r);
            }
            let fast = a.matter.admission.room_scans;
            let slow = b.matter.admission.room_scans;
            let count = a.battle.waves.len();
            apply_waves(&mut a);
            apply_scalar_waves(&mut b);
            assert_eq!(a.matter.admission.room_scans - fast, u64::from(count > 0));
            assert_eq!(b.matter.admission.room_scans - slow, count as u64);
            assert_eq!(a.state_hash(), b.state_hash(), "cap={cap} tick={tick}");
            assert_eq!(saved_contacts(&a), saved_contacts(&b));
            assert_eq!(a.matter.admission.body, b.matter.admission.body);
            assert_eq!(a.matter.admission.ground, b.matter.admission.ground);
            assert!(a.matter.economy.ledger.balanced());
        }
        if cap > 0 {
            assert!(a.battle.waves.is_empty());
            assert!(a.units.iter().all(|u| u.removed.len() == 10));
        } else {
            assert_eq!(a.battle.waves.len(), 3);
            assert!(a.units.iter().all(|u| u.removed.is_empty()));
        }
    }
}

fn add_ground_waves(r: &mut BlastRange) {
    let p = [40, 2, 40];
    assert_ne!(material(&r.world, p), 0);
    // All profiles share geometry here, but retain distinct energy and fragment
    // boundaries. Include annihilation and a genuinely unaffordable contact.
    r.battle.energy_config = r.battle.shell_config;
    r.battle.energy_config.fragment_box = 4;
    r.battle.canister_config = r.battle.shell_config;
    r.battle.canister_config.fragment_box = 1;
    r.battle.anni_config = r.battle.shell_config;
    r.battle.anni_config.material_energy_j = 0;
    for weapon in [
        Weapon::Energy,
        Weapon::Canister,
        Weapon::Annihilation,
        Weapon::Blast,
    ] {
        let config = match weapon {
            Weapon::Energy => &r.battle.energy_config,
            Weapon::Canister => &r.battle.canister_config,
            Weapon::Annihilation => &r.battle.anni_config,
            _ => &r.battle.shell_config,
        };
        let mut wave = Explosion::with_damage(&r.world, p, config, &r.units, &r.surface_work);
        wave.wave.advance_contacts(usize::MAX);
        for x in 0..3 {
            wave.pending.push_back(phys::blast::Hit {
                cell: [
                    p[0] + x - wave.origin[0],
                    p[1] - wave.origin[1],
                    p[2] - wave.origin[2],
                ],
                pressure: u32::MAX,
                broken: true,
            });
        }
        r.battle.waves.push(Burst {
            side: None,
            weapon,
            wave,
            energy: if weapon == Weapon::Blast {
                0
            } else {
                u64::from(u32::MAX) * 1000
            },
        });
    }
}

#[test]
fn wave_phase_capacity_preserves_mixed_ground_claims_and_partial_confirmation() {
    for cap in [1, 3, 64] {
        let mut a = contested_waves();
        let mut b = contested_waves();
        for r in [&mut a, &mut b] {
            r.matter.tuning.max_flying = cap;
            r.matter.tuning.max_claims = 1;
            // A pending edit must continue reserving its slot across phases.
            r.world.max_edits_per_tick = 1;
            add_ground_waves(r);
        }
        for tick in 3..27 {
            for r in [&mut a, &mut b] {
                r.tick = tick;
                settle_admitted(r);
            }
            apply_waves(&mut a);
            apply_scalar_waves(&mut b);
            assert_eq!(a.state_hash(), b.state_hash(), "cap={cap} tick={tick}");
            assert_eq!(saved_contacts(&a), saved_contacts(&b));
            assert_eq!(a.matter.admission.body, b.matter.admission.body);
            assert_eq!(a.matter.admission.ground, b.matter.admission.ground);
            for r in [&mut a, &mut b] {
                r.world.flush();
                r.matter.debris.confirm(
                    r.world.completed(),
                    &mut r.matter.economy,
                    &r.matter.tuning,
                );
                assert!(r.matter.economy.ledger.balanced());
            }
            assert_eq!(a.state_hash(), b.state_hash());
        }
        assert!(a.matter.admission.ground[0] > 0, "no real ground claim");
        assert!(a.matter.admission.ground[1] > 0, "no unaffordable contact");
        if cap < 64 {
            assert!(a.matter.admission.ground[2] > 0, "no pool deferral");
        }
        assert!(a.matter.admission.room_scans < b.matter.admission.room_scans);
    }
}

#[test]
fn ground_admission_reports_the_first_actual_gate_without_spending_on_retries() {
    let mut r = contested_waves();
    let p = [40, 2, 40];
    let q = [41, 2, 40];
    assert_ne!(material(&r.world, p), 0);
    let m = &mut r.matter;
    m.tuning.max_flying = 64;
    m.tuning.max_fragments = 64;
    m.tuning.max_claims = 1;
    for bad in [[-1, 2, 40], [r.world.nx as i32, 2, 40], [0, 2, 40]] {
        assert!(matches!(
            m.ground(&mut r.world, bad, p),
            matter::ClaimResult::Indestructible
        ));
    }
    assert!(matches!(
        m.ground(&mut r.world, p, p),
        matter::ClaimResult::Unaffordable
    ));
    m.begin(u32::MAX, 2);
    assert!(matches!(
        m.ground(&mut r.world, p, p),
        matter::ClaimResult::Applied
    ));
    let paid = (m.remaining_mj, m.spent_mj);
    // Claim cap wins over duplicate; pool pressure wins over claim cap.
    for at in [p, q] {
        assert!(matches!(
            m.ground(&mut r.world, at, p),
            matter::ClaimResult::Deferred
        ));
    }
    m.tuning.max_claims = 10;
    assert!(matches!(
        m.ground(&mut r.world, p, p),
        matter::ClaimResult::Deferred
    ));
    m.tuning.max_claims = 1;
    m.tuning.max_flying = 1;
    assert!(matches!(
        m.ground(&mut r.world, q, p),
        matter::ClaimResult::Deferred
    ));
    assert_eq!(m.admission.ground, [1, 1, 1, 2, 1, 3]);
    assert_eq!((m.remaining_mj, m.spent_mj), paid);
    assert_eq!(m.debris.claims(), 1);
    assert!(m.economy.ledger.balanced());
}
