use super::*;
use crate::blast::{
    battle::commands::{Command, CommandPhase, MotionField, Producer},
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
fn legacy(t: &mut Blast, field: DebrisField, value: u32) {
    match field {
        DebrisField::DragPct => t.drag_pct = value.min(100),
        DebrisField::BouncePct => t.bounce_pct = value.min(99),
        DebrisField::FrictionPct => t.friction_pct = value.min(100),
        DebrisField::SleepMmS => t.sleep_mm_s = value.max(1),
        DebrisField::PushPct => t.push_pct = value,
        DebrisField::PushUpPct => t.push_up_pct = value.min(100),
        DebrisField::RestCheckTicks => t.rest_check_ticks = value.max(1),
        DebrisField::PileCell => t.pile_cell = value.max(1),
        DebrisField::ScatterPct => t.scatter_pct = value.min(100),
        DebrisField::SpeedMinPct => t.speed_min_pct = value.min(100),
        DebrisField::SmallPiecePct => t.small_piece_pct = value.min(100),
        DebrisField::LiftPct => t.lift_pct = value.min(100),
        DebrisField::MotionPct => t.motion_pct = value.min(99),
        DebrisField::MaxFlying => t.max_flying = value.clamp(1, t.max_fragments),
    }
}

#[cfg(test)]
#[test]
fn debris_commands_match_legacy_clamps_and_record_one_result_per_edit() {
    let mut r = arena();
    r.start_battle_command_capture(100).unwrap();
    let mut reference = r.matter.tuning;
    for (code, field) in DebrisField::ALL.into_iter().enumerate() {
        assert_eq!(DebrisField::from_code(code as u8), Some(field));
        for value in [0, 17, u32::MAX] {
            let before = reference;
            legacy(&mut reference, field, value);
            let command = Command::TuneDebris { field, value };
            let expected = if reference == before {
                Outcome::Unchanged
            } else {
                Outcome::Applied
            };
            assert_eq!(r.execute_command(command), expected);
            assert_eq!(r.matter.tuning, reference);
            assert_eq!(r.execute_command(command), Outcome::Unchanged);
        }
    }
    assert_eq!(DebrisField::from_code(14), None);
    let trace = r.finish_battle_command_capture().unwrap();
    assert!(trace.complete());
    assert_eq!(trace.records.len(), 84);
    for (i, record) in trace.records.iter().enumerate() {
        assert_eq!(record.sequence, i as u64);
        assert_eq!(record.phase, CommandPhase::BetweenTicks);
        assert_eq!(record.producer, Producer::Player);
    }
    // No panic or accidental admission when an internal test closes the pool.
    r.matter.tuning.max_fragments = 0;
    r.execute_command(Command::TuneDebris {
        field: DebrisField::MaxFlying,
        value: 123,
    });
    assert_eq!(r.matter.tuning.max_flying, 0);
}

#[cfg(test)]
#[test]
fn all_debris_configuration_fields_affect_hash_and_restore_with_legacy_motion_formats() {
    let mut r = arena();
    let stock = r.snapshot();
    let stock_hash = r.state_hash();
    let base = values(&r.matter.tuning);
    for index in 0..32 {
        let mut changed = base;
        changed[index] = if index == 10 { 3 } else { changed[index] + 1 };
        r.matter.tuning = from_values(changed);
        assert_ne!(r.state_hash(), stock_hash, "field {index}");
        let bytes = r.snapshot();
        assert_eq!(u32::from_le_bytes(bytes[..4].try_into().unwrap()), 55);
        let restored = BlastRange::restore(r.config.clone(), &bytes).unwrap();
        assert_eq!(r.matter.tuning, restored.matter.tuning, "field {index}");
        assert_eq!(bytes, restored.snapshot());
    }
    r.matter.tuning = r.debris_defaults;
    assert_eq!(stock, r.snapshot());
    r.execute_command(Command::TuneMotion {
        kind: RangeUnitKind::Tank,
        include_gun: true,
        field: MotionField::Speed,
        value: 19,
    });
    let motion_only = r.snapshot();
    assert_eq!(u32::from_le_bytes(motion_only[..4].try_into().unwrap()), 54);
    r.execute_command(Command::TuneDebris {
        field: DebrisField::DragPct,
        value: 17,
    });
    let restored = BlastRange::restore(r.config.clone(), &r.snapshot()).unwrap();
    assert_eq!(r.snapshot(), restored.snapshot());
    assert_eq!(r.gun.motion.speed, restored.gun.motion.speed);
    r.matter.tuning = r.debris_defaults;
    assert_eq!(motion_only, r.snapshot());
}

#[cfg(test)]
fn prefix(bytes: &[u8]) -> usize {
    let mut s = Scan::new(bytes);
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
#[test]
fn debris_prelude_rejects_empty_redundant_truncated_and_invalid_motion_flags() {
    let mut r = arena();
    r.execute_command(Command::TuneDebris {
        field: DebrisField::DragPct,
        value: 17,
    });
    let bytes = r.snapshot();
    let at = prefix(&bytes);
    for (offset, value) in [(0, 0u32), (4, r.debris_defaults.drag_pct)] {
        let mut bad = bytes.clone();
        bad[at + offset..at + offset + 4].copy_from_slice(&value.to_le_bytes());
        assert!(BlastRange::restore(r.config.clone(), &bad).is_none());
    }
    let mut bad = bytes.clone();
    bad[at + 8] = 2;
    assert!(BlastRange::restore(r.config.clone(), &bad).is_none());
    for n in 0..=8 {
        assert!(BlastRange::restore(r.config.clone(), &bytes[..at + n]).is_none());
    }
    for case in 0..3 {
        r.matter.tuning = r.debris_defaults;
        match case {
            0 => r.matter.tuning.pile_cell = 0,
            1 => r.matter.tuning.motion_pct = 100,
            _ => r.matter.tuning.fragment_box = 5,
        }
        assert!(BlastRange::restore(r.config.clone(), &r.snapshot()).is_none());
    }
}

#[cfg(test)]
#[test]
fn debris_storage_ceiling_loads_before_fragment_validation_and_lower_flying_cap_keeps_matter() {
    use crate::{burst::Fragment, matter::Kind};
    use fx::Fx;
    use phys::V3;
    let mut r = arena();
    let count = r.matter.tuning.max_fragments + 1;
    r.matter.tuning.max_fragments = count;
    for id in 1..=count {
        let held =
            r.matter
                .economy
                .ledger
                .create(Kind::Debris, r.matter.economy.per_voxel(3), u64::MAX);
        r.matter.debris.fragments.push(Fragment {
            id: u64::from(id),
            position: V3::new(Fx::from_int(70), Fx::from_int(50), Fx::from_int(70)),
            velocity: V3::ZERO,
            material: 3,
            paint: None,
            mm: 100,
            basis: crate::burst::IDENTITY,
            mass_g: 7850,
            count: 1,
            shape: 1,
            side: 1,
            chunk: None,
            held,
            resting: false,
        });
    }
    r.matter.debris.next_id = u64::from(count);
    let ledger = r.matter.economy.ledger.snapshot();
    let held_before: u64 = ledger.0.iter().map(|c| c.held).sum();
    r.execute_command(Command::TuneDebris {
        field: DebrisField::MaxFlying,
        value: 1,
    });
    assert_eq!(r.matter.debris.fragments.len(), count as usize);
    assert_eq!(r.matter.debris.fragment_room(&r.matter.tuning), 0);
    let bytes = r.snapshot();
    let restored = BlastRange::restore(r.config.clone(), &bytes).unwrap();
    assert_eq!(r.matter.tuning, restored.matter.tuning);
    assert_eq!(bytes, restored.snapshot());
    assert_eq!(restored.matter.debris.fragments.len(), count as usize);
    assert_eq!(
        restored
            .matter
            .debris
            .fragment_pools(&restored.matter.tuning)
            .flying,
        count as usize
    );
    assert!(restored.matter.economy.ledger.balanced());
    assert_eq!(
        held_before,
        restored
            .matter
            .economy
            .ledger
            .snapshot()
            .0
            .iter()
            .map(|c| c.held)
            .sum()
    );
    // A reader using stock caps would refuse precisely this same physical body.
    let at = prefix(&bytes);
    let end = at + 4 + 2 * 4 + 1; // two tuning fields and the false motion flag
    let mut legacy = bytes.clone();
    legacy.drain(at..end);
    legacy[..4].copy_from_slice(&53u32.to_le_bytes());
    assert!(BlastRange::restore(r.config.clone(), &legacy).is_none());
}
