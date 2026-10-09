use super::*;
use crate::blast::{
    battle::commands::{Command, CommandPhase, Producer},
    ExplosionConfig,
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
fn edit(kind: RangeUnitKind, include_gun: bool, field: MotionField, value: i32) -> Command {
    Command::TuneMotion {
        kind,
        include_gun,
        field,
        value,
    }
}

// Independent reference to the pre-command UI setter, including its clamps.
#[cfg(test)]
fn legacy(m: &mut Motion, field: MotionField, v: i32) {
    match field {
        MotionField::Speed => m.speed = v.max(1),
        MotionField::Step => m.step = v.clamp(1, 8),
        MotionField::Stiffness => m.chassis.stiffness = v,
        MotionField::Damping => m.chassis.damping = v,
        MotionField::TravelCm => m.chassis.travel_cm = v,
        MotionField::ClearanceCm => m.chassis.clearance_cm = v,
        MotionField::Inertia => m.chassis.inertia = v,
        MotionField::AngularDamping => m.chassis.ang_damp = v,
        MotionField::MaxTiltPct => m.chassis.max_tilt_pct = v,
        MotionField::ContactCm => m.chassis.contact_cm = v,
        MotionField::ApproachPct => m.chassis.approach_pct = v,
        MotionField::SettlePct => m.chassis.settle_pct = v,
        MotionField::SettleTiltPct => m.chassis.settle_tilt_pct = v,
        MotionField::Thrust => m.chassis.thrust = v,
        MotionField::Brake => m.chassis.brake = v,
        MotionField::Turn => m.chassis.turn = v,
        MotionField::Climb => m.chassis.climb = v,
        MotionField::Descend => m.chassis.descend = v,
    }
}

#[cfg(test)]
#[test]
fn all_fields_match_legacy_group_edits_and_roundtrip_without_changing_stock_bytes() {
    let mut r = arena();
    let original = r.snapshot();
    let defaults = r.motion_defaults.clone();
    let mut reference = BlastRange::restore(r.config.clone(), &original).unwrap();
    for (index, field) in MotionField::ALL.into_iter().enumerate() {
        assert_eq!(MotionField::from_code(index as u8), Some(field));
        let value = if field == MotionField::Step {
            8
        } else {
            defaults[0][index] + 7
        };
        assert_eq!(
            r.execute_command(edit(RangeUnitKind::Tank, true, field, value)),
            Outcome::Applied
        );
        for u in reference
            .units
            .iter_mut()
            .chain(std::iter::once(&mut reference.gun))
        {
            legacy(&mut u.motion, field, value);
        }
        assert_eq!(r.snapshot(), reference.snapshot());
        assert_ne!(
            r.state_hash(),
            BlastRange::restore(r.config.clone(), &original)
                .unwrap()
                .state_hash()
        );
        let bytes = r.snapshot();
        assert_eq!(u32::from_le_bytes(bytes[..4].try_into().unwrap()), 54);
        let restored = BlastRange::restore(r.config.clone(), &bytes).unwrap();
        assert_eq!(bytes, restored.snapshot());
        assert_eq!(
            r.execute_command(edit(RangeUnitKind::Tank, true, field, value)),
            Outcome::Unchanged
        );
    }
    assert_eq!(MotionField::from_code(18), None);
    for (u, base) in r
        .units
        .iter_mut()
        .chain(std::iter::once(&mut r.gun))
        .zip(defaults)
    {
        for (field, value) in MotionField::ALL.into_iter().zip(base) {
            legacy(&mut u.motion, field, value);
        }
    }
    assert_eq!(original, r.snapshot());
}

#[cfg(test)]
#[test]
fn targets_include_dead_slots_both_sides_and_explicit_gun_with_one_receipt() {
    let mut r = arena();
    r.units[0].status.alive = false;
    let gun_speed = r.gun.motion.speed;
    r.start_battle_command_capture(10).unwrap();
    assert_eq!(
        r.execute_command(edit(RangeUnitKind::Tank, false, MotionField::Speed, 0)),
        Outcome::Applied
    );
    assert!(r.units.iter().all(|u| u.motion.speed == 1));
    assert_eq!(r.gun.motion.speed, gun_speed);
    assert_eq!(
        r.execute_command(edit(RangeUnitKind::Strider, false, MotionField::Step, 99)),
        Outcome::Rejected
    );
    // The separately addressed gun need not have the selected roster kind.
    assert_eq!(
        r.execute_command(edit(RangeUnitKind::Strider, true, MotionField::Step, 99)),
        Outcome::Applied
    );
    assert_eq!(r.gun.motion.step, 8);
    assert_eq!(
        r.execute_command(edit(RangeUnitKind::Strider, true, MotionField::Step, 99)),
        Outcome::Unchanged
    );
    let trace = r.finish_battle_command_capture().unwrap();
    assert!(trace.complete());
    assert_eq!(trace.records.len(), 4);
    for (i, record) in trace.records.iter().enumerate() {
        assert_eq!(record.sequence, i as u64);
        assert_eq!(record.producer, Producer::Player);
        assert_eq!(record.phase, CommandPhase::BetweenTicks);
        assert_eq!((record.requested_tick, record.assigned_tick), (0, 0));
    }
}

#[cfg(test)]
#[test]
fn tuning_tail_rejects_noncanonical_ids_masks_defaults_and_truncation() {
    let mut r = arena();
    r.execute_command(edit(RangeUnitKind::Tank, false, MotionField::Speed, 123));
    let bytes = r.snapshot();
    let mut tail = Blob::new();
    r.save_motion_tuning(&mut tail);
    assert_eq!(tail.len(), 28); // two one-field roster deltas
    let offset = bytes.len() - tail.len();
    assert_eq!(&bytes[offset..], tail.v);
    for (at, value) in [
        (0, 0u32),
        (0, u32::MAX),
        (4, 3),
        (8, 0),
        (8, 1 << 18),
        (12, r.motion_defaults[0][0] as u32),
        (16, 0),
    ] {
        let mut bad = bytes.clone();
        bad[offset + at..offset + at + 4].copy_from_slice(&value.to_le_bytes());
        assert!(
            BlastRange::restore(r.config.clone(), &bad).is_none(),
            "offset {at}, value {value}"
        );
    }
    for omitted in 1..=tail.len() {
        assert!(BlastRange::restore(r.config.clone(), &bytes[..bytes.len() - omitted]).is_none());
    }
}
