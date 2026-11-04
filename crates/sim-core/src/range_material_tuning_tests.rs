use super::*;
use crate::blast::{
    battle::commands::{Command, CommandPhase, DebrisField, MotionField, Producer},
    ExplosionConfig, RangeUnitKind,
};

#[cfg(test)]
fn arena() -> BlastRange {
    let mut layout =
        modding::range::RangeLayout::from_ron(include_str!("../../../data/range-cover.ron"))
            .unwrap();
    layout.battle = true;
    layout.size = [640, 192, 640];
    layout.blocks.clear();
    layout.shields.clear();
    layout.course = None;
    layout.clear_min = [64, 64];
    layout.clear_max = [600, 600];
    layout.units = [(170, true), (450, false)]
        .map(|(x, allied)| modding::range::RangeUnitDef {
            weapon: None,
            kind: RangeUnitKind::Tank,
            at: [x, 16, 336],
            allied,
        })
        .to_vec();
    BlastRange::with_layout(
        ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap(),
        layout,
    )
    .unwrap()
}

#[cfg(test)]
#[test]
fn material_commands_match_registry_validation_and_leave_accounted_matter_unchanged() {
    let mut r = arena();
    let mut reference = arena();
    let mut physical = Blob::new();
    r.matter.save(&mut physical);
    r.start_battle_command_capture(100).unwrap();
    for field in [MaterialField::Strength, MaterialField::Melt] {
        for material in [0, 1, 3, 8, 10, u16::MAX] {
            for value in [0, 17, u32::MAX] {
                let before = reference.matter.economy.mats.get(material).copied();
                let result = before.map(|mut d| {
                    match field {
                        MaterialField::Strength => d.strength = value.max(1),
                        MaterialField::Melt => d.melt = value,
                        MaterialField::Density => unreachable!(),
                    }
                    reference.matter.economy.mats.replace(d)
                });
                let after = reference.matter.economy.mats.get(material).copied();
                let expected = match result {
                    Some(Ok(())) if after == before => Outcome::Unchanged,
                    Some(Ok(())) => Outcome::Applied,
                    _ => Outcome::Rejected,
                };
                let command = Command::TuneMaterial {
                    material,
                    field,
                    value,
                };
                assert_eq!(r.execute_command(command), expected);
                assert_eq!(r.matter.economy.mats.get(material).copied(), after);
                assert_eq!(
                    r.execute_command(command),
                    if expected == Outcome::Rejected {
                        expected
                    } else {
                        Outcome::Unchanged
                    }
                );
            }
        }
    }
    let trace = r.finish_battle_command_capture().unwrap();
    assert!(trace.complete());
    assert_eq!(trace.records.len(), 72);
    for (i, record) in trace.records.iter().enumerate() {
        assert_eq!(record.sequence, i as u64);
        assert_eq!(record.phase, CommandPhase::BetweenTicks);
        assert_eq!(record.producer, Producer::Player);
    }
    let mut after = Blob::new();
    r.matter.save(&mut after);
    assert_eq!(physical.v, after.v);
    assert!(r.matter.economy.ledger.balanced());
    assert_eq!(MaterialField::from_code(0), Some(MaterialField::Strength));
    assert_eq!(MaterialField::from_code(1), Some(MaterialField::Melt));
    assert_eq!(MaterialField::from_code(2), Some(MaterialField::Density));
    assert_eq!(MaterialField::from_code(3), None);
}

#[cfg(test)]
#[test]
fn material_properties_hash_and_restore_with_all_motion_and_debris_combinations() {
    for motion in [false, true] {
        for debris in [false, true] {
            let mut r = arena();
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
            let stock = r.snapshot();
            let stock_hash = r.state_hash();
            for material in [0, 1, 3, 8] {
                for field in [MaterialField::Strength, MaterialField::Melt] {
                    // Air cannot gain strength, but its melt field is accepted
                    // by the same registry contract as the old setter.
                    if material == 0 && field == MaterialField::Strength {
                        continue;
                    }
                    let original = *r.matter.economy.mats.get(material).unwrap();
                    assert_eq!(
                        r.execute_command(Command::TuneMaterial {
                            material,
                            field,
                            value: 17
                        }),
                        Outcome::Applied
                    );
                    assert_ne!(r.state_hash(), stock_hash);
                    let bytes = r.snapshot();
                    assert_eq!(u32::from_le_bytes(bytes[8..12].try_into().unwrap()), 56);
                    let restored = BlastRange::restore(r.config, &bytes).unwrap();
                    assert_eq!(bytes, restored.snapshot());
                    assert_eq!(
                        r.matter.economy.mats.hash(),
                        restored.matter.economy.mats.hash()
                    );
                    r.matter.economy.mats.replace(original).unwrap();
                    assert_eq!(stock, r.snapshot());
                }
            }
        }
    }
}

#[cfg(test)]
fn prefix(bytes: &[u8]) -> usize {
    let mut s = Scan::new(&bytes[8..]);
    s.u32();
    s.bool();
    s.bool();
    s.u32();
    if s.bool() {
        s.u8s();
    }
    assert!(s.ok());
    bytes.len() - s.left()
}

#[cfg(test)]
fn density(r: &mut BlastRange, material: Mat, value: u32) -> Outcome {
    r.execute_command(Command::TuneMaterial {
        material,
        field: MaterialField::Density,
        value,
    })
}

#[cfg(test)]
#[test]
fn density_changes_preserve_stock_validate_inputs_and_keep_history_after_reset() {
    let mut r = arena();
    let stock = r.snapshot();
    let mut matter = Blob::new();
    r.matter.save(&mut matter);
    for (material, value, expected) in [
        (0, 1, Outcome::Rejected),
        (10, 1, Outcome::Rejected),
        (3, modding::MAX_DENSITY + 1, Outcome::Rejected),
        (3, 785, Outcome::Unchanged),
    ] {
        assert_eq!(density(&mut r, material, value), expected);
        assert_eq!(stock, r.snapshot());
    }
    for value in [0, modding::MAX_DENSITY, 785] {
        assert_eq!(density(&mut r, 3, value), Outcome::Applied);
        assert_eq!(r.matter.economy.mats.get(3).unwrap().density, value.max(1));
        let mut after = Blob::new();
        r.matter.save(&mut after);
        assert_eq!(matter.v, after.v, "command must not revalue or sink stock");
        let bytes = r.snapshot();
        assert_eq!(u32::from_le_bytes(bytes[8..12].try_into().unwrap()), 57);
        assert_eq!(
            bytes,
            BlastRange::restore(r.config, &bytes).unwrap().snapshot()
        );
    }
    let bytes = r.snapshot();
    let at = prefix(&bytes);
    assert_eq!(
        &bytes[at..at + 4],
        &0u32.to_le_bytes(),
        "empty override list after reset"
    );
    assert_ne!(bytes, stock, "history is authoritative even after reset");
}

#[cfg(test)]
#[test]
fn density_prelude_rejects_invalid_density_masks_and_legacy_reinterpretation() {
    let mut r = arena();
    density(&mut r, 1, 121);
    let bytes = r.snapshot();
    let at = prefix(&bytes);
    for value in [0, 120, modding::MAX_DENSITY + 1] {
        let mut bad = bytes.clone();
        bad[at + 7..at + 11].copy_from_slice(&value.to_le_bytes());
        assert!(BlastRange::restore(r.config, &bad).is_none());
    }
    for mask in [0, 8, 255] {
        let mut bad = bytes.clone();
        bad[at + 6] = mask;
        assert!(BlastRange::restore(r.config, &bad).is_none());
    }
    let mut old = bytes.clone();
    old[8..12].copy_from_slice(&56u32.to_le_bytes());
    assert!(BlastRange::restore(r.config, &old).is_none());
    for end in at..at + 13 {
        assert!(BlastRange::restore(r.config, &bytes[..end]).is_none());
    }
}

#[cfg(test)]
fn extract(r: &mut BlastRange, p: [u32; 3]) {
    assert!(r.matter.melt(&mut r.world, p));
    r.world.flush();
    r.matter
        .debris
        .confirm(r.world.completed(), &mut r.matter.economy, &r.matter.tuning);
}

#[cfg(test)]
#[test]
fn density_terrain_claims_use_confirmation_density_and_mixed_piles_keep_matter_on_reclaim() {
    use crate::{burst::Surroundings, matter::Kind};
    use fx::Fx;
    use phys::V3;
    let mut r = arena();
    let p = [240, 15, 240];
    let mat = r.world.material(p[0], p[1], p[2]);
    assert_ne!(mat, 0);
    let base = r.matter.economy.mats.get(mat).unwrap().density;
    extract(&mut r, p);
    let first = r.matter.debris.fragments[0].mass_g;
    let q = [241, 15, 240];
    assert_eq!(r.world.material(q[0], q[1], q[2]), mat);
    assert!(r.matter.melt(&mut r.world, q));
    density(&mut r, mat, base + 1);
    let mut r = BlastRange::restore(r.config, &r.snapshot()).unwrap();
    assert_eq!(r.matter.debris.claims(), 1);
    r.world.flush();
    r.matter
        .debris
        .confirm(r.world.completed(), &mut r.matter.economy, &r.matter.tuning);
    assert_eq!(r.matter.debris.fragments.len(), 2);
    let total = u64::from(base) * 2 + 1;
    let mass = r
        .matter
        .debris
        .fragments
        .iter()
        .map(|f| f.mass_g)
        .sum::<u64>();
    assert!(mass > first * 2);
    assert_eq!(
        r.matter
            .debris
            .fragments
            .iter()
            .map(|f| r.matter.economy.ledger.held(f.held))
            .sum::<u64>(),
        total
    );
    // Put the real extracted pieces face-to-face on the same supported site.
    // Mere membership of a coarse pile cell must not teleport/reshape shards.
    for (i, f) in r.matter.debris.fragments.iter_mut().enumerate() {
        f.position = V3::new(
            Fx::from_ratio(501, 2).add(Fx::from_int(i as i32)),
            Fx::from_ratio(33, 2),
            Fx::from_ratio(501, 2),
        );
        f.velocity = V3::ZERO;
        f.basis = crate::burst::IDENTITY;
        f.resting = true;
    }
    r.matter.debris.step_fragments(
        &r.world,
        &mut r.matter.economy,
        &r.matter.tuning,
        Fx::ZERO,
        0,
        Surroundings::NONE,
    );
    assert_eq!(
        r.matter.debris.fragments.len(),
        1,
        "must actually merge different birth densities"
    );
    let f = &r.matter.debris.fragments[0];
    assert_eq!(
        (f.count, f.mass_g, r.matter.economy.ledger.held(f.held)),
        (2, mass, total)
    );
    density(&mut r, mat, base);
    let mut r = BlastRange::restore(r.config, &r.snapshot()).unwrap();
    let f = &r.matter.debris.fragments[0];
    assert_eq!(
        (f.count, f.mass_g, r.matter.economy.ledger.held(f.held)),
        (2, mass, total)
    );
    let held = f.held;
    // Isolate the legacy quantity validator from header/version parsing.
    let mut saved = Blob::new();
    r.matter.save(&mut saved);
    let mut old = crate::blast::matter::Matter::new(&r.units);
    assert!(old
        .load(&mut Scan::new(&saved.v), &r.world, &r.units)
        .is_none());
    let mut current = crate::blast::matter::Matter::new(&r.units);
    current.density_history = true;
    assert!(current
        .load(&mut Scan::new(&saved.v), &r.world, &r.units)
        .is_some());
    // Lose the support under the actual mixed pile; waking keeps its stock.
    for x in 248..254 {
        for z in 248..254 {
            for y in 12..16 {
                r.world.queue_dig(x, y, z);
            }
        }
    }
    r.world.flush();
    let mut woke = false;
    for tick in 0..=u64::from(r.matter.tuning.rest_check_ticks) {
        r.matter.debris.step_fragments(
            &r.world,
            &mut r.matter.economy,
            &r.matter.tuning,
            Fx::from_ratio(1, 100),
            tick,
            Surroundings::NONE,
        );
        let f = &r.matter.debris.fragments[0];
        woke |= !f.resting;
        assert_eq!(
            (f.mass_g, r.matter.economy.ledger.held(f.held)),
            (mass, total)
        );
    }
    assert!(
        woke,
        "the mixed pile must actually wake after losing support"
    );
    // Reclaim operates on resting wrecks. Let the woken pile actually land
    // before transferring it, rather than reclaiming a fragment in flight.
    for tick in 100..600 {
        r.matter.debris.step_fragments(
            &r.world,
            &mut r.matter.economy,
            &r.matter.tuning,
            Fx::from_ratio(1, 100),
            tick,
            Surroundings::NONE,
        );
        let f = &r.matter.debris.fragments[0];
        assert_eq!(
            (f.mass_g, r.matter.economy.ledger.held(f.held)),
            (mass, total)
        );
        if f.resting {
            break;
        }
    }
    assert!(r.matter.debris.fragments[0].resting);
    let tank = r.matter.economy.ledger.create(Kind::Hauler, 0, u64::MAX);
    assert_eq!(
        r.matter.economy.ledger.transfer(held, tank, u64::MAX),
        total
    );
    r.matter.debris.sweep(&r.matter.economy);
    assert!(r.matter.debris.fragments.is_empty());
    assert_eq!(r.matter.economy.ledger.held(tank), total);
    assert!(r.matter.economy.ledger.balanced());
}

#[cfg(test)]
#[test]
fn density_snapshot_combines_all_material_fields_motion_and_debris() {
    for motion in [false, true] {
        for debris in [false, true] {
            let mut r = arena();
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
            for field in MaterialField::ALL {
                r.execute_command(Command::TuneMaterial {
                    material: 3,
                    field,
                    value: 23,
                });
            }
            let bytes = r.snapshot();
            assert_eq!(
                bytes,
                BlastRange::restore(r.config, &bytes).unwrap().snapshot()
            );
        }
    }
}

#[cfg(test)]
#[test]
fn density_restore_rejects_impossible_quantities_even_in_balanced_ledgers() {
    use crate::matter::{Kind, Ledger};
    let mut r = arena();
    density(&mut r, 1, 121);
    extract(&mut r, [240, 15, 240]);
    let bytes = r.snapshot();
    for case in 0..5 {
        let mut forged = BlastRange::restore(r.config, &bytes).unwrap();
        let id = forged.matter.debris.fragments[0].held as usize;
        let (mut containers, sunk, mut initial, head, free) =
            forged.matter.economy.ledger.snapshot();
        match case {
            0 | 1 => {
                let value = if case == 0 {
                    0
                } else {
                    u64::from(modding::MAX_DENSITY) + 1
                };
                // Deliberately forge a self-consistent ledger so the physical
                // quantity check, rather than balance alone, rejects the save.
                initial = initial - containers[id].held + value;
                containers[id].held = value;
            }
            2 => containers[id].kind = Kind::Hauler,
            3 => forged.matter.debris.fragments[0].material = u16::MAX,
            4 => {
                forged.matter.debris.fragments[0].paint = Some([1, 0, 0]);
                forged.matter.debris.fragments[0].material = 3;
            }
            _ => unreachable!(),
        }
        forged.matter.economy.ledger =
            Ledger::from_snapshot(containers, sunk, initial, head, free).unwrap();
        assert!(forged.matter.economy.ledger.balanced());
        assert!(
            BlastRange::restore(forged.config, &forged.snapshot()).is_none(),
            "case {case}"
        );
    }
}

#[cfg(test)]
#[test]
fn density_body_melt_fracture_and_annihilation_use_original_stock_quantities_and_mass() {
    use crate::blast::matter::ClaimResult;
    for value in [1, modding::MAX_DENSITY] {
        let mut r = arena();
        density(&mut r, 3, value);
        let held = r.matter.body_container(0);
        let initial = r.matter.economy.ledger.held(held);
        let cells: Vec<_> = (0..usize::from(r.units[0].body.vol.nz))
            .flat_map(|z| {
                (0..usize::from(r.units[0].body.vol.ny))
                    .flat_map(move |y| (0..30).map(move |x| [x, y, z]))
            })
            .filter(|p| r.units[0].body.vol.get(p[0], p[1], p[2]))
            .take(3)
            .collect();
        assert_eq!(cells.len(), 3);
        assert!(r.matter.melt_unit(0, &r.units[0], 0, cells[0]));
        r.matter.begin(1_000_000, 1);
        let u = &r.units[0];
        assert!(matches!(
            r.matter.crush(0, u, 0, cells[1], &u.pose),
            ClaimResult::Applied
        ));
        r.matter.annihilate_body(0, 1);
        for p in cells {
            assert!(r.units[0].body.remove(p[0], p[1], p[2]));
            r.units[0].removed.push(p.map(|v| v as u8));
        }
        assert_eq!(r.matter.economy.ledger.held(held), initial - 3 * 785);
        assert_eq!(r.matter.debris.fragments.len(), 2);
        for f in &r.matter.debris.fragments {
            assert_eq!(r.matter.economy.ledger.held(f.held), 785);
            assert_eq!(f.mass_g, 7850);
        }
        assert_eq!(
            r.matter
                .economy
                .ledger
                .sunk(crate::matter::Sink::Annihilation),
            785
        );
        let bytes = r.snapshot();
        assert_eq!(
            bytes,
            BlastRange::restore(r.config, &bytes).unwrap().snapshot()
        );
        assert!(r.matter.economy.ledger.balanced());
    }
}

#[cfg(test)]
#[test]
fn material_prelude_rejects_bad_counts_ids_order_masks_values_flags_and_truncation() {
    let mut r = arena();
    let default_strength = r.matter.economy.mats.get(1).unwrap().strength;
    r.execute_command(Command::TuneMaterial {
        material: 1,
        field: MaterialField::Strength,
        value: if default_strength == modding::MAX_STRENGTH {
            default_strength - 1
        } else {
            default_strength + 1
        },
    });
    r.execute_command(Command::TuneMaterial {
        material: 3,
        field: MaterialField::Melt,
        value: 11,
    });
    let bytes = r.snapshot();
    let at = prefix(&bytes);
    assert!(BlastRange::restore(r.config, &bytes).is_some());
    for (offset, value) in [
        (0, 0u32),
        (0, u32::MAX),
        (7, 0),
        (7, modding::MAX_STRENGTH + 1),
        (7, default_strength),
    ] {
        let mut bad = bytes.clone();
        bad[at + offset..at + offset + 4].copy_from_slice(&value.to_le_bytes());
        assert!(
            BlastRange::restore(r.config, &bad).is_none(),
            "offset {offset} value {value}"
        );
    }
    // Unknown ID; duplicate/out-of-order second ID; invalid property masks;
    // invalid debris and motion presence flags.
    for (offset, value) in [(4, 10), (11, 1), (11, 0), (6, 0), (6, 4), (18, 2), (19, 2)] {
        let mut bad = bytes.clone();
        bad[at + offset] = value;
        assert!(
            BlastRange::restore(r.config, &bad).is_none(),
            "offset {offset}"
        );
    }
    for end in at..=at + 19 {
        assert!(BlastRange::restore(r.config, &bytes[..end]).is_none());
    }
}
