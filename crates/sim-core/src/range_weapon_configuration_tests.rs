use super::*;
use crate::blast::battle::commands::{Command, DebrisField, MaterialField, MotionField};
use crate::blast::RangeUnitKind;

#[cfg(test)]
fn stock() -> ExplosionConfig {
    ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap()
}

#[cfg(test)]
#[test]
fn weapon_configuration_without_layout_keeps_original_world_and_witness_after_reset() {
    let initial = stock();
    let mut r = BlastRange::new(initial);
    let mut c = initial;
    c.unit_ablation_resistance = 0;
    c.edits_per_tick = 1;
    c.gravity += 1;
    assert_eq!(
        r.execute_command(Command::ConfigureWeapon { config: c }),
        Outcome::Applied
    );
    let mut copy = BlastRange::restore(c, &r.snapshot()).unwrap();
    assert_eq!(r.snapshot(), copy.snapshot());
    assert_eq!(r.weapon_initial, initial);
    assert_eq!(r.world.max_edits_per_tick, copy.world.max_edits_per_tick);
    for _ in 0..20 {
        r.step();
        copy.step();
        assert_eq!(r.snapshot(), copy.snapshot());
    }
    r.execute_command(Command::ConfigureWeapon { config: initial });
    assert_eq!(
        r.snapshot(),
        BlastRange::restore(c, &r.snapshot()).unwrap().snapshot()
    );
}

#[cfg(test)]
fn arena() -> BlastRange {
    let mut layout =
        modding::range::RangeLayout::from_ron(include_str!("../../../data/range-cover.ron"))
            .unwrap();
    layout.size = [640, 192, 640];
    layout.blocks.clear();
    layout.shields.clear();
    layout.course = None;
    layout.units.truncate(1);
    layout.units[0].at = [300, 16, 336];
    BlastRange::with_layout(stock(), layout).unwrap()
}

#[cfg(test)]
#[test]
fn weapon_configuration_rejects_atomically_normalizes_layout_and_records_once() {
    let mut layout = arena().layout.unwrap();
    layout.battle = true;
    let mut r = BlastRange::with_layout(stock(), layout).unwrap();
    let original = r.config;
    let bytes = r.snapshot();
    r.start_battle_command_capture(8).unwrap();
    let mut c = original;
    c.unit_ablation_resistance = u32::MAX; // Layout owns this field.
    assert_eq!(
        r.execute_command(Command::ConfigureWeapon { config: c }),
        Outcome::Unchanged
    );
    assert_eq!(bytes, r.snapshot());
    c.energy = 0;
    assert_eq!(
        r.execute_command(Command::ConfigureWeapon { config: c }),
        Outcome::Rejected
    );
    assert_eq!(bytes, r.snapshot());
    c.energy = original.energy + 1;
    assert_eq!(
        r.execute_command(Command::ConfigureWeapon { config: c }),
        Outcome::Applied
    );
    assert_eq!(
        r.config.unit_ablation_resistance,
        original.unit_ablation_resistance
    );
    r.reconfigure(original);
    let trace = r.finish_battle_command_capture().unwrap();
    assert_eq!(trace.records.len(), 4);
    assert_eq!(
        trace.records.iter().map(|r| r.outcome).collect::<Vec<_>>(),
        [
            Outcome::Unchanged,
            Outcome::Rejected,
            Outcome::Applied,
            Outcome::Applied
        ]
    );
    assert!(r.weapon_configured);
    assert_eq!(r.config, original);
    assert_ne!(bytes, r.snapshot()); // Ownership survives reset.
    let mut wrong = stock();
    wrong.gravity = 9876;
    assert_eq!(
        r.snapshot(),
        BlastRange::restore(wrong, &r.snapshot())
            .unwrap()
            .snapshot()
    );
}

#[cfg(test)]
#[test]
fn weapon_configuration_snapshot_owns_constructor_and_live_config_with_every_tuning_prefix() {
    for material in 0..3 {
        for motion in [false, true] {
            for debris in [false, true] {
                let mut r = arena();
                if material > 0 {
                    r.execute_command(Command::TuneMaterial {
                        material: 3,
                        field: if material == 1 {
                            MaterialField::Strength
                        } else {
                            MaterialField::Density
                        },
                        value: 321,
                    });
                }
                if motion {
                    r.execute_command(Command::TuneMotion {
                        kind: RangeUnitKind::Tank,
                        include_gun: true,
                        field: MotionField::Speed,
                        value: 19,
                    });
                }
                if debris {
                    r.execute_command(Command::TuneDebris {
                        field: DebrisField::DragPct,
                        value: 17,
                    });
                }
                let mut c = r.config;
                c.energy += 23;
                c.edits_per_tick = 13;
                assert_eq!(
                    r.execute_command(Command::ConfigureWeapon { config: c }),
                    Outcome::Applied
                );
                let bytes = r.snapshot();
                assert_eq!(u32::from_le_bytes(bytes[8..12].try_into().unwrap()), 58);
                let mut wrong = stock();
                wrong.edits_per_tick = 1;
                wrong.unit_ablation_resistance = 0;
                wrong.gravity = 9000;
                let copy = BlastRange::restore(wrong, &bytes).unwrap();
                assert_eq!(bytes, copy.snapshot());
                assert_eq!(r.state_hash(), copy.state_hash());
                assert_eq!(r.config, copy.config);
                assert_eq!(r.weapon_initial, copy.weapon_initial);
                assert_eq!(r.world.max_edits_per_tick, copy.world.max_edits_per_tick);
                assert_ne!(copy.world.max_edits_per_tick, c.edits_per_tick);
                assert_eq!(r.matter.density_history, copy.matter.density_history);
            }
        }
    }
}

#[cfg(test)]
#[test]
fn weapon_configuration_wire_roundtrips_all_presets_and_rejects_malformed_structure() {
    for text in [
        include_str!("../../../data/explosion.ron"),
        include_str!("../../../data/kinetic-shell.ron"),
        include_str!("../../../data/laser.ron"),
        include_str!("../../../data/flamethrower.ron"),
        include_str!("../../../data/machine-gun.ron"),
    ] {
        let c = ExplosionConfig::from_ron(text).unwrap();
        let mut b = Blob::new();
        save(&c, &mut b);
        let mut s = Scan::new(&b.v);
        assert_eq!(load(&mut s), Some(c));
        assert_eq!(s.left(), 0);
        for end in 0..b.v.len() {
            assert!(
                load(&mut Scan::new(&b.v[..end])).is_none(),
                "truncation {end}"
            );
        }
        let prefix = if c.soil_depth_cost == 0 && c.soil_fracture_milli == 1000 {
            0
        } else {
            9
        };
        for offset in [0, prefix, prefix + 1, prefix + 98] {
            // Delivery, motion, ricochet option.
            let mut bad = b.v.clone();
            bad[offset] = 255;
            assert!(load(&mut Scan::new(&bad)).is_none(), "tag {offset}");
        }
    }
}

#[cfg(test)]
#[test]
fn weapon_configuration_applied_snapshot_rejects_bad_configs_prefixes_and_truncation() {
    let mut r = arena();
    let mut c = r.config;
    c.energy += 1;
    r.execute_command(Command::ConfigureWeapon { config: c });
    let bytes = r.snapshot();
    let mut s = Scan::new(&bytes[18..]);
    load(&mut s).unwrap();
    let current = bytes.len() - s.left();
    load(&mut s).unwrap();
    let after_configs = bytes.len() - s.left();
    assert!(s.bool());
    s.u8s();
    let prefix = bytes.len() - s.left();
    let depth_prefix = if c.soil_depth_cost == 0 && c.soil_fracture_milli == 1000 {
        0
    } else {
        9
    };
    for at in [18 + depth_prefix + 18, current + depth_prefix + 18] {
        // energy is a gameplay value.
        let mut bad = bytes.clone();
        bad[at..at + 4].copy_from_slice(&0u32.to_le_bytes());
        assert!(BlastRange::restore(stock(), &bad).is_none());
    }
    for at in [prefix, prefix + 1, prefix + 2] {
        let mut bad = bytes.clone();
        bad[at] = 2;
        assert!(BlastRange::restore(stock(), &bad).is_none(), "prefix {at}");
    }
    for end in 18..=after_configs {
        assert!(BlastRange::restore(stock(), &bytes[..end]).is_none());
    }
}

#[cfg(test)]
#[test]
fn weapon_configuration_during_flight_and_wave_restores_exact_continuation() {
    let mut r = arena();
    r.control();
    r.set_auto_attack(true);
    let mut copy = None;
    let mut flight = false;
    let mut wave = false;
    for _ in 0..360 {
        if let Some(b) = &mut copy {
            BlastRange::step(b);
        }
        r.step();
        if let Some(b) = &copy {
            assert_eq!(r.snapshot(), b.snapshot());
        }
        let change = (!flight && r.shell.is_some()) || (!wave && r.explosion.is_some());
        if change {
            flight |= r.shell.is_some();
            wave |= r.explosion.is_some();
            let mut c = r.config;
            c.energy += 11;
            c.capture_cells_per_tick = 1000;
            c.wave_work_per_tick = 64;
            r.execute_command(Command::ConfigureWeapon { config: c });
            let mut wrong = stock();
            wrong.delivery = Delivery::Flame;
            copy = Some(BlastRange::restore(wrong, &r.snapshot()).unwrap());
            assert_eq!(r.snapshot(), copy.as_ref().unwrap().snapshot());
        }
        assert!(r.matter.economy.ledger.balanced());
    }
    assert!(flight && wave, "must exercise live flight and wave");
    assert!(
        r.units[0].revision > 0 || r.removed > 0,
        "must actually damage matter"
    );
}

#[test]
fn soil_depth_wire_extension_and_legacy_configs_roundtrip() {
    for depth in [0, 140, 1_000_000] {
        let mut c = stock();
        c.soil_depth_cost = depth;
        let mut bytes = Blob::new();
        save(&c, &mut bytes);
        assert_eq!(bytes.v[0] == 128, depth != 0);
        let mut scan = Scan::new(&bytes.v);
        assert_eq!(load(&mut scan), Some(c));
        assert_eq!(scan.left(), 0);
    }
}

#[test]
fn material_work_wire_extension_roundtrips_presets_and_pending_invalid_values() {
    for depth in [0, 260] {
        let mut c =
            ExplosionConfig::from_ron(include_str!("../../../data/nuclear-warhead.ron")).unwrap();
        // Exercise the previous material-work extension independently.
        c.soil_crater = None;
        c.soil_depth_cost = depth;
        let mut b = Blob::new();
        save(&c, &mut b);
        assert_eq!(b.v[0], 129);
        let mut scan = Scan::new(&b.v);
        assert_eq!(load(&mut scan), Some(c));
        assert_eq!(scan.left(), 0);
        for end in 0..b.v.len() {
            assert!(load(&mut Scan::new(&b.v[..end])).is_none());
        }
        for (index, value) in [(0, 1), (4, 0), (8, u32::MAX)] {
            let mut invalid = c;
            invalid.material_resistance.as_mut().unwrap()[index] = value;
            assert!(invalid.validate().is_err());
            let mut b = Blob::new();
            save(&invalid, &mut b);
            // Pending commands decode gameplay values, then reject at admission.
            assert_eq!(load(&mut Scan::new(&b.v)), Some(invalid));
        }
    }
    let c = stock();
    assert!(c.material_resistance.is_none());
    let mut b = Blob::new();
    save(&c, &mut b);
    assert_ne!(b.v[0], 129);
    assert_eq!(load(&mut Scan::new(&b.v)), Some(c));
}

#[test]
fn soil_crater_wire_extension_owns_shape_and_preserves_legacy_configs() {
    let c = ExplosionConfig::from_ron(include_str!("../../../data/nuclear-warhead.ron")).unwrap();
    let mut b = Blob::new();
    save(&c, &mut b);
    assert_eq!(b.v[0], 130);
    let mut scan = Scan::new(&b.v);
    assert_eq!(load(&mut scan), Some(c));
    assert_eq!(scan.left(), 0);
    for end in 0..b.v.len() {
        assert!(load(&mut Scan::new(&b.v[..end])).is_none());
    }
    let mut legacy = c;
    legacy.soil_crater = None;
    let mut old = Blob::new();
    save(&legacy, &mut old);
    assert_eq!(&b.v[33..], old.v.as_slice());
    assert_eq!(load(&mut Scan::new(&old.v)), Some(legacy));
    for (offset, value) in [
        (1, 0),
        (5, 0),
        (9, 0),
        (17, 0),
        (21, u32::MAX),
        (25, 0),
        (29, 0),
    ] {
        let mut invalid = b.v.clone();
        invalid[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        let pending = load(&mut Scan::new(&invalid)).expect("decode invalid pending shape");
        assert!(pending.validate().is_err());
    }
}
