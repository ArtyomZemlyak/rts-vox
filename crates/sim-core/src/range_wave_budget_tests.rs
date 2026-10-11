//! Small legal RON quotas must not freeze a crowded battle's waves.
use super::*;

fn crowded_waves(count: usize, capture: usize, propagation: usize) -> BlastRange {
    let layout = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
    let config = ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    let mut r = BlastRange::with_layout(config, layout).unwrap();
    let text = include_str!("../../../data/explosion.ron")
        .replace("half_cells: 20", "half_cells: 2")
        .replace("cell_units: 2", "cell_units: 1")
        .replace("energy: 3500", "energy: 1")
        .replace("material_energy_j: 45000000", "material_energy_j: 0")
        .replace(
            "capture_cells_per_tick: 49152",
            &format!("capture_cells_per_tick: {capture}"),
        )
        .replace(
            "wave_work_per_tick: 32768",
            &format!("wave_work_per_tick: {propagation}"),
        );
    // These are accepted mod configurations, not invalid zero budgets.
    r.battle.shell_config = ExplosionConfig::from_ron(&text).unwrap();
    assert_eq!(r.battle.shell_config.capture_cells_per_tick, capture);
    assert_eq!(r.battle.shell_config.wave_work_per_tick, propagation);
    // The tick's ceiling as small as one wave's budget: the legal quota
    // is smaller than the queue, as the shared quota was (ADR-681).
    ceilings(&mut r, capture, propagation);
    for i in 0..count {
        r.battle.waves.push(Burst {
            side: None,
            weapon: Weapon::Blast,
            wave: Explosion::begin(
                [320 + i as i32 * 5, 120, 336],
                &r.battle.shell_config,
                &r.units,
            ),
            energy: 0,
        });
    }
    r
}

fn ceilings(r: &mut BlastRange, capture: usize, work: usize) {
    let t = std::sync::Arc::make_mut(&mut r.battle.tuning);
    t.wave_capture_ceiling = capture;
    t.wave_work_ceiling = work;
    t.wave_contact_ceiling = work;
}

fn apply(r: &mut BlastRange) {
    let mut b = std::mem::replace(&mut r.battle, Battle::new(&r.units, 1));
    r.battle_waves(&mut b, &mut |_| {});
    r.battle = b;
    r.tick += 1;
}

#[test]
fn crowded_waves_capture_even_when_the_legal_budget_is_smaller_than_the_queue() {
    for count in [3, 16] {
        let mut r = crowded_waves(count, 1, 1);
        for _ in 0..count {
            apply(&mut r);
        }
        assert_eq!(r.battle.waves.len(), count);
        assert!(
            r.battle.waves.iter().all(|w| w.wave.captured > 0),
            "a legal capture quota of one froze {count} waves"
        );
        for _ in 0..1200 {
            apply(&mut r);
            if r.battle.waves.is_empty() {
                break;
            }
        }
        assert!(
            r.battle.waves.is_empty(),
            "capture/propagation never completed"
        );
        assert!(r.matter.economy.ledger.balanced());
    }
}

#[test]
fn crowded_waves_propagate_and_release_capacity_with_a_small_legal_budget() {
    let mut r = crowded_waves(16, 40000, 1);
    for _ in 0..32 {
        apply(&mut r);
        if r.battle.waves.is_empty() {
            break;
        }
    }
    assert!(
        r.battle.waves.is_empty(),
        "captured waves held admission forever"
    );
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn crowded_wave_budget_turn_survives_loading_a_partial_capture() {
    let mut a = crowded_waves(3, 2, 1);
    let mut b = crowded_waves(3, 2, 1);
    for _ in 0..2 {
        apply(&mut a);
        apply(&mut b);
    }
    assert!(
        a.battle.waves.iter().any(|w| w.wave.captured > 0),
        "no partial capture to save"
    );
    assert!(a.battle.waves.iter().all(|w| w.wave.capturing()));
    let mut bytes = Blob::new();
    b.battle.save(&mut bytes);
    b.battle.save_wave_contacts(&mut bytes);
    let mut scan = Scan::new(&bytes.v);
    let mut restored = Battle::new(&b.units, 1);
    restored.shell_config = b.battle.shell_config;
    // The ceilings are data, not saved state: the restored battle runs the
    // same tuning (ADR-681).
    restored.tuning = b.battle.tuning.clone();
    let allied: Vec<_> = (0..b.units.len()).map(|id| b.allied(id)).collect();
    restored
        .load(
            &mut scan,
            &b.units,
            b.size,
            b.turret_limits,
            (52, b.tick, &allied),
        )
        .unwrap();
    restored.load_wave_contacts(&mut scan, &b.units).unwrap();
    assert!(scan.ok() && scan.left() == 0);
    b.battle = restored;
    assert_eq!(a.state_hash(), b.state_hash());
    for _ in 0..160 {
        apply(&mut a);
        apply(&mut b);
        assert_eq!(a.state_hash(), b.state_hash(), "saved budget turn changed");
        if a.battle.waves.is_empty() {
            break;
        }
    }
    assert!(a.battle.waves.is_empty());
}

/// Re-qualified for ADR-681: the shared quota split into shares is gone;
/// each wave takes its preset's own budget while the tick's ceiling over
/// all waves lasts. The ceiling still caps the tick, a wave past it waits
/// and is counted, and the rotation brings every wave into service.
#[test]
fn wave_ceilings_hold_the_tick_and_rotate_every_wave_into_service() {
    for count in [1, 3, 16] {
        for ceiling in [1, 2, 5] {
            let mut r = crowded_waves(count, 1, 1);
            ceilings(&mut r, ceiling, ceiling);
            let mut served = vec![0; count];
            for _ in 0..count {
                let before: Vec<_> = r.battle.waves.iter().map(|w| w.wave.captured).collect();
                apply(&mut r);
                assert_eq!(r.battle.waves.len(), count);
                let mut charged = 0;
                for (k, w) in r.battle.waves.iter().enumerate() {
                    if w.wave.captured > before[k] {
                        served[k] += 1;
                        charged += 1;
                    }
                }
                assert!(charged <= ceiling, "the ceiling did not cap the tick");
                assert_eq!(r.battle.wave_held[0], count.saturating_sub(ceiling));
            }
            assert!(
                served.iter().all(|&s| s >= 1),
                "a wave never came into service: {served:?}"
            );
        }
    }
}

/// A wave's side survives a snapshot (version 60), and a wave restored
/// from an older one has none and counts against neither half (ADR-649).
#[test]
fn wave_sides_survive_a_snapshot() {
    let mut r = crowded_waves(3, 1, 1);
    r.battle.waves[0].side = Some(true);
    r.battle.waves[1].side = Some(false);
    let saved = r.snapshot();
    assert_eq!(u32::from_le_bytes(saved[..4].try_into().unwrap()), 60);
    let restored = BlastRange::restore(r.config, &saved).unwrap();
    let sides: Vec<_> = restored.battle.waves.iter().map(|w| w.side).collect();
    assert_eq!(sides, [Some(true), Some(false), None]);
    assert_eq!(restored.state_hash(), r.state_hash());
    assert_eq!(restored.snapshot(), saved);
    // Waves with no side save in an older format without the section, as
    // before version 60, and come back with none: they count against the
    // whole ceiling only.
    for w in &mut r.battle.waves {
        w.side = None;
    }
    let older = r.snapshot();
    assert!(u32::from_le_bytes(older[..4].try_into().unwrap()) < 60);
    let restored = BlastRange::restore(r.config, &older).unwrap();
    assert!(restored.battle.waves.iter().all(|w| w.side.is_none()));
}
