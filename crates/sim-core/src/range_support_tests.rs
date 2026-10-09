use super::*;

fn scene(kind: RangeUnitKind) -> BlastRange {
    let c = ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
    let mut layout = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
    layout.course = None;
    layout.blocks.clear();
    layout.clear_min = [64, 64];
    layout.clear_max = [600, 600];
    layout.units.truncate(1);
    layout.units[0].kind = kind;
    let mut r = BlastRange::with_layout(c, layout).unwrap();
    r.control();
    r
}

#[test]
fn parked_tank_falls_when_its_floor_is_removed() {
    let mut r = scene(RangeUnitKind::Tank);
    let [x, _, z] = r.units[0].at;
    for px in x - 45..x + 45 {
        for pz in z - 35..z + 35 {
            for py in 8..16 {
                r.world.queue_dig(px as u32, py, pz as u32);
            }
        }
    }
    r.world.flush();
    for _ in 0..90 {
        r.step();
    }
    assert!(
        r.units[0].motion.pos[1] < Fx::from_int(10),
        "tank floats at {:?}",
        r.units[0].motion.pos
    );
    assert!(r.units[0].motion.pos[1] > Fx::from_int(6));
}

#[test]
fn paused_patrols_do_not_pause_a_tanks_fall() {
    let mut r = scene(RangeUnitKind::Tank);
    r.patrols_running = false;
    let level = r.units[0].motion.hull.y;
    r.units[0].motion.hull.y = level.add(Fx::from_int(24));
    r.units[0].motion.hull.vy = Fx::ZERO;
    for _ in 0..90 {
        r.step();
    }
    assert!(
        r.units[0].motion.hull.y.sub(level).abs() < Fx::ONE,
        "frozen airborne hull"
    );
}

#[test]
fn blast_shove_reaches_the_next_tank_and_replays_after_a_snapshot() {
    let c = ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
    let mut layout = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
    layout.course = None;
    layout.blocks.clear();
    layout.clear_min = [64, 64];
    layout.clear_max = [600, 600];
    layout.units = [300, 361]
        .map(|x| modding::range::RangeUnitDef {
            weapon: None,
            allied: false,
            kind: RangeUnitKind::Tank,
            at: [x, FLOOR, 336],
        })
        .to_vec();
    let mut outcomes = Vec::new();
    for reverse in [false, true] {
        if reverse {
            layout.units.reverse();
        }
        let (source, target) = if reverse { (1, 0) } else { (0, 1) };
        let mut a = BlastRange::with_layout(c, layout.clone()).unwrap();
        a.patrols_running = false;
        for _ in 0..20 {
            a.step();
        }
        let original = a.units[target].motion.pos[0];
        let body = a.units[source].body.clone();
        a.units[source]
            .motion
            .impulse(&body, [254, FLOOR, 336], 120_000);
        for _ in 0..3 {
            a.step();
        }
        let mut b = BlastRange::restore(c, &a.snapshot()).unwrap();
        for _ in 0..90 {
            a.step();
            b.step();
            assert_eq!(a.state_hash(), b.state_hash());
            let p = motion::Obstacle::of(&a.units[source].motion, &a.units[source].body);
            let q = motion::Obstacle::of(&a.units[target].motion, &a.units[target].body);
            assert!(
                q.pos[0].sub(p.pos[0]) >= p.half[0].add(q.half[0]),
                "tanks entered each other"
            );
        }
        assert!(
            a.units[target].motion.pos[0] > original.add(Fx::ONE),
            "the second tank received no contact shove: {:?}",
            a.units[target].motion.pos
        );
        outcomes.push((a.units[source].motion.pos[0], a.units[target].motion.pos[0]));
    }
    assert!(
        outcomes[0].0.sub(outcomes[1].0).abs() < Fx::ONE
            && outcomes[0].1.sub(outcomes[1].1).abs() < Fx::ONE,
        "contact depends on slot order: {outcomes:?}"
    );
}

#[test]
fn a_driving_tank_pushes_a_parked_tank_without_overlap_or_replay_drift() {
    let mut c = ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
    c.energy = 2700;
    let mut layout = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
    layout.course = None;
    layout.blocks.clear();
    layout.clear_min = [64, 64];
    layout.clear_max = [600, 600];
    layout.units = [300, 361]
        .map(|x| modding::range::RangeUnitDef {
            weapon: None,
            allied: false,
            kind: RangeUnitKind::Tank,
            at: [x, FLOOR, 336],
        })
        .to_vec();
    let mut outcomes = Vec::new();
    for reverse in [false, true] {
        if reverse {
            layout.units.reverse();
        }
        let (source, target) = if reverse { (1, 0) } else { (0, 1) };
        let mut a = BlastRange::with_layout(c, layout.clone()).unwrap();
        a.patrols_running = true;
        for _ in 0..20 {
            a.step();
        }
        let parked = a.units[target].motion.pos[0];
        let mover = &mut a.units[source].motion;
        mover.enabled = true;
        mover.destination = Some([450, 336]);
        mover.path = vec![[450, 336]];
        for _ in 0..90 {
            a.step();
            let p = motion::Obstacle::of(&a.units[source].motion, &a.units[source].body);
            let q = motion::Obstacle::of(&a.units[target].motion, &a.units[target].body);
            assert!(q.pos[0].sub(p.pos[0]) >= p.half[0].add(q.half[0]));
        }
        let mut b = BlastRange::restore(c, &a.snapshot()).unwrap();
        for _ in 0..180 {
            a.step();
            b.step();
            assert_eq!(a.state_hash(), b.state_hash());
            let p = motion::Obstacle::of(&a.units[source].motion, &a.units[source].body);
            let q = motion::Obstacle::of(&a.units[target].motion, &a.units[target].body);
            assert!(q.pos[0].sub(p.pos[0]) >= p.half[0].add(q.half[0]));
        }
        assert!(
            a.units[target].motion.pos[0] > parked.add(Fx::ONE),
            "mover {:?}, target {:?}, start {parked:?}",
            a.units[source].motion.pos,
            a.units[target].motion.pos
        );
        outcomes.push((a.units[source].motion.pos[0], a.units[target].motion.pos[0]));
    }
    assert!(
        outcomes[0].0.sub(outcomes[1].0).abs() < Fx::ONE
            && outcomes[0].1.sub(outcomes[1].1).abs() < Fx::ONE,
        "motor contact depends on slot order: {outcomes:?}"
    );
}

#[test]
fn a_convoy_contact_uses_its_closing_speed() {
    let c = ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
    let mut layout = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
    layout.course = None;
    layout.blocks.clear();
    layout.clear_min = [64, 64];
    layout.clear_max = [600, 600];
    layout.units = [300, 361]
        .map(|x| modding::range::RangeUnitDef {
            weapon: None,
            allied: false,
            kind: RangeUnitKind::Tank,
            at: [x, FLOOR, 336],
        })
        .to_vec();
    let mut push = Vec::new();
    for moving_target in [false, true] {
        let mut a = BlastRange::with_layout(c, layout.clone()).unwrap();
        a.patrols_running = true;
        for _ in 0..20 {
            a.step();
        }
        let source = &mut a.units[0].motion;
        source.enabled = true;
        source.destination = Some([500, 336]);
        source.path = vec![[500, 336]];
        source.drive.vel = Fx::from_int(40);
        if moving_target {
            let target = &mut a.units[1].motion;
            target.enabled = true;
            target.destination = Some([500, 336]);
            target.path = vec![[500, 336]];
            target.drive.vel = Fx::from_int(20);
        }
        a.step();
        push.push(a.units[1].motion.shove.x);
        let p = motion::Obstacle::of(&a.units[0].motion, &a.units[0].body);
        let q = motion::Obstacle::of(&a.units[1].motion, &a.units[1].body);
        assert!(q.pos[0].sub(p.pos[0]) >= p.half[0].add(q.half[0]));
        let mut replay = BlastRange::restore(c, &a.snapshot()).unwrap();
        for _ in 0..8 {
            a.step();
            replay.step();
            assert_eq!(a.state_hash(), replay.state_hash());
        }
    }
    assert!(push[0] > Fx::ZERO, "no contact with parked hull: {push:?}");
    assert!(
        push[1] > Fx::ZERO && push[1] < push[0],
        "a following hull received the full parked-hull impact: {push:?}"
    );
}

#[test]
fn head_on_driving_contact_is_slot_independent() {
    let c = ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
    let mut layout = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
    layout.course = None;
    layout.blocks.clear();
    layout.clear_min = [64, 64];
    layout.clear_max = [600, 600];
    layout.units = [300, 361]
        .map(|x| modding::range::RangeUnitDef {
            weapon: None,
            allied: false,
            kind: RangeUnitKind::Tank,
            at: [x, FLOOR, 336],
        })
        .to_vec();
    let mut outcomes = Vec::new();
    for reverse in [false, true] {
        if reverse {
            layout.units.reverse();
        }
        let (left, right) = if reverse { (1, 0) } else { (0, 1) };
        let mut a = BlastRange::with_layout(c, layout.clone()).unwrap();
        a.patrols_running = true;
        for _ in 0..20 {
            a.step();
        }
        let mover = &mut a.units[left].motion;
        mover.enabled = true;
        mover.destination = Some([500, 336]);
        mover.path = vec![[500, 336]];
        mover.drive.vel = Fx::from_int(40);
        let mover = &mut a.units[right].motion;
        mover.enabled = true;
        mover.destination = Some([100, 336]);
        mover.path = vec![[100, 336]];
        mover.drive.head = fx::angle::FULL / 2;
        mover.drive.vel = Fx::from_int(40);
        a.step();
        let p = motion::Obstacle::of(&a.units[left].motion, &a.units[left].body);
        let q = motion::Obstacle::of(&a.units[right].motion, &a.units[right].body);
        assert!(q.pos[0].sub(p.pos[0]) >= p.half[0].add(q.half[0]));
        assert!(a.units[left].motion.shove.x < Fx::ZERO);
        assert!(a.units[right].motion.shove.x > Fx::ZERO);
        let mut replay = BlastRange::restore(c, &a.snapshot()).unwrap();
        for _ in 0..60 {
            a.step();
            replay.step();
            assert_eq!(a.state_hash(), replay.state_hash());
            let p = motion::Obstacle::of(&a.units[left].motion, &a.units[left].body);
            let q = motion::Obstacle::of(&a.units[right].motion, &a.units[right].body);
            assert!(q.pos[0].sub(p.pos[0]) >= p.half[0].add(q.half[0]));
        }
        outcomes.push((a.units[left].motion.pos[0], a.units[right].motion.pos[0]));
    }
    assert!(
        outcomes[0].0.sub(outcomes[1].0).abs() < Fx::ONE
            && outcomes[0].1.sub(outcomes[1].1).abs() < Fx::ONE,
        "head-on contact depends on slot order: {outcomes:?}"
    );
}

#[test]
fn solo_shooter_and_target_exchange_contact_shove() {
    let c = ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
    for gun_is_source in [false, true] {
        let mut layout =
            RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
        layout.course = None;
        layout.blocks.clear();
        layout.clear_min = [64, 64];
        layout.clear_max = [600, 600];
        layout.gun = [if gun_is_source { 300 } else { 361 }, FLOOR, 336];
        layout.units.truncate(1);
        layout.units[0].at = [if gun_is_source { 361 } else { 300 }, FLOOR, 336];
        let mut r = BlastRange::with_layout(c, layout).unwrap();
        r.patrols_running = false;
        for _ in 0..20 {
            r.step();
        }
        let original = if gun_is_source {
            r.units[0].motion.pos[0]
        } else {
            r.gun.motion.pos[0]
        };
        if gun_is_source {
            let body = r.gun.body.clone();
            r.gun.motion.impulse(&body, [254, FLOOR, 336], 120_000);
        } else {
            let body = r.units[0].body.clone();
            r.units[0].motion.impulse(&body, [254, FLOOR, 336], 120_000);
        }
        for _ in 0..90 {
            r.step();
        }
        let target = if gun_is_source {
            r.units[0].motion.pos[0]
        } else {
            r.gun.motion.pos[0]
        };
        assert!(target > original.add(Fx::ONE), "no transfer to target");
    }
}

#[test]
fn a_nearby_wall_cannot_lift_a_parked_tank() {
    let c = ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
    let layout = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
    let mut r = BlastRange::with_layout(c, layout).unwrap();
    r.control();
    r.patrols_running = false;
    for _ in 0..120 {
        r.step();
        assert!(
            r.units[0].motion.pos[1].sub(Fx::from_int(FLOOR)).abs() < Fx::ONE,
            "wall became a floor: {:?}",
            r.units[0].motion.floors
        );
    }
}

#[test]
fn destroying_the_core_does_not_disable_structural_separation() {
    let mut r = scene(RangeUnitKind::Tank);
    let u = &mut r.units[0];
    let (cx, cy, cz) = u.body.core;
    let mut count = 0;
    for z in cz..cz + u.body.core_size {
        for y in cy..cy + u.body.core_size {
            for x in cx..cx + u.body.core_size {
                if u.body.remove(x as usize, y as usize, z as usize) {
                    u.removed.push([x, y, z]);
                    count += 1;
                }
            }
        }
    }
    r.matter.annihilate_body(0, count);
    r.units[0].refresh_damage(0);
    assert!(!r.units[0].status.alive);
    assert!(
        r.units[0].section.is_some(),
        "dead root stopped shedding geometry"
    );
    let mut restored = BlastRange::restore(r.config, &r.snapshot()).unwrap();
    for _ in 0..180 {
        r.step();
        restored.step();
        assert_eq!(r.state_hash(), restored.state_hash());
    }
    assert_eq!(r.units[0].body.solid_count(), 0);
    assert!(r.matter.debris.fragments.iter().any(|f| f.chunk.is_some()));
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn pristine_parts_are_attached_and_a_melted_plate_sheds_only_its_loose_half() {
    for kind in [RangeUnitKind::Tank, RangeUnitKind::Strider] {
        let mut r = scene(kind);
        r.units[0].refresh_damage(0);
        assert!(
            r.units[0].seg_sections.iter().all(Option::is_none),
            "intact {kind:?} sheds parts"
        );
    }
    let mut r = scene(RangeUnitKind::Tank);
    let before = r.units[0].body.segments[0].body.solid_count();
    let (ny, nz) = {
        let b = &r.units[0].body.segments[0].body;
        (b.vol.ny, b.vol.nz)
    };
    r.thermal_tuning.cooling_milli = 0;
    r.thermal_tuning.conduction_milli = 0;
    for z in 0..nz {
        for y in 0..ny {
            r.thermals.deposit(
                thermal::Cell {
                    unit: 0,
                    part: 1,
                    at: [4, y, z],
                },
                4000,
                &r.thermal_tuning,
            );
        }
    }
    r.step_unit_heat();
    assert_eq!(r.thermals.melted, u64::from(ny) * u64::from(nz));
    assert!(r.units[0].seg_sections[0].is_some());
    assert!(!r.units[0].seg_gone[0], "attached half lost its weld");
    for _ in 0..120 {
        r.step();
    }
    let u = &r.units[0];
    assert_eq!(u.body.segments[0].body.solid_count(), before / 2);
    assert!(!u.seg_gone[0]);
    let parts: Vec<_> = r
        .matter
        .debris
        .fragments
        .iter()
        .filter(|f| f.chunk.as_ref().is_some_and(|c| c.segment == Some(0)))
        .collect();
    assert_eq!(parts.len(), 1);
    let piece = parts[0];
    assert_eq!(piece.count, 3 * u32::from(ny) * u32::from(nz));
    assert!(piece.resting, "loose plate never landed");
    let c = piece.chunk.as_ref().unwrap();
    assert!(
        piece.position.y < c.centre.y.sub(Fx::ONE),
        "loose plate hangs in the air"
    );
    assert!(r.matter.economy.ledger.balanced());
}

/// **A thrown section tumbles and lands on its turned box** (ADR-468).
/// The loose half of a melted plate leaves unturned: its cells are placed
/// by the segment frame it had, and a basis of that frame's axes turned
/// it twice on screen. Thrown sideways it turns as it flies, slower than
/// a crumb, and comes to rest with the bottom of its turned box on the
/// ground; dropped, it does not turn at all.
#[test]
fn a_thrown_section_tumbles_and_lands_on_its_turned_box() {
    for thrown in [false, true] {
        let mut r = scene(RangeUnitKind::Tank);
        let (ny, nz) = {
            let b = &r.units[0].body.segments[0].body;
            (b.vol.ny, b.vol.nz)
        };
        r.thermal_tuning.cooling_milli = 0;
        r.thermal_tuning.conduction_milli = 0;
        for z in 0..nz {
            for y in 0..ny {
                r.thermals.deposit(
                    thermal::Cell {
                        unit: 0,
                        part: 1,
                        at: [4, y, z],
                    },
                    4000,
                    &r.thermal_tuning,
                );
            }
        }
        r.step_unit_heat();
        let section = |r: &BlastRange| {
            r.matter
                .debris
                .fragments
                .iter()
                .position(|f| f.chunk.as_ref().is_some_and(|c| c.segment == Some(0)))
        };
        let mut i = None;
        for _ in 0..120 {
            r.step();
            i = section(&r);
            if i.is_some() {
                break;
            }
        }
        let i = i.expect("the plate never left");
        // A piece turns about the axis its number picks; take a number, still
        // unique, that turns this one about a level axis, where the turn
        // shows in how it lands.
        let id = r.matter.debris.next_id + 3 - r.matter.debris.next_id % 3;
        r.matter.debris.next_id = id;
        let f = &mut r.matter.debris.fragments[i];
        f.id = id;
        assert_eq!(f.basis, crate::burst::IDENTITY, "a section leaves unturned");
        assert!(!f.resting);
        if thrown {
            f.velocity.x = Fx::from_ratio(90, crate::TICKS_PER_SECOND);
            f.velocity.y = Fx::from_ratio(60, crate::TICKS_PER_SECOND);
        }
        let mut turned = false;
        for _ in 0..600 {
            r.step();
            let f = r
                .matter
                .debris
                .fragments
                .iter()
                .find(|f| f.id == id)
                .unwrap();
            turned |= f.basis != crate::burst::IDENTITY;
            if f.resting {
                break;
            }
        }
        let f = r
            .matter
            .debris
            .fragments
            .iter()
            .find(|f| f.id == id)
            .unwrap();
        assert!(f.resting, "the section never landed");
        assert_eq!(turned, thrown, "thrown {thrown}: turned {turned}");
        // Rows of a rotation stay unit length and square to each other.
        let dot = |a: phys::V3, b: phys::V3| a.x.mul(b.x).add(a.y.mul(b.y)).add(a.z.mul(b.z));
        for a in 0..3 {
            assert!(dot(f.basis[a], f.basis[a]).sub(Fx::ONE).abs() < Fx::from_ratio(1, 100));
            assert!(dot(f.basis[a], f.basis[(a + 1) % 3]).abs() < Fx::from_ratio(1, 100));
        }
        // On the ground by its turned box, not sunk into it or hanging:
        // the lowest corner of the section's box as its basis turns it.
        let (lo, hi) = f.chunk.as_ref().unwrap().bounds;
        let bottom = (0..8)
            .map(|corner: usize| {
                (0..3).fold(f.position.y, |y, a| {
                    let v = if corner & (1 << a) == 0 { lo[a] } else { hi[a] };
                    y.add(f.basis[a].y.mul(v))
                })
            })
            .min()
            .unwrap();
        let floor = Fx::from_int(r.world.column_ceiling(
            f.position.x.to_int_trunc() as u32,
            f.position.z.to_int_trunc() as u32,
        ) as i32);
        assert!(
            bottom.sub(floor).abs() <= Fx::ONE,
            "thrown {thrown}: box bottom {bottom:?} on a floor at {floor:?}"
        );
        assert!(r.matter.economy.ledger.balanced());
    }
}

#[test]
fn partial_segment_extraction_waits_for_room_and_replays_after_loading() {
    let mut r = scene(RangeUnitKind::Strider);
    let leg = r.units[0].body.legs[0];
    let mounts = r.units[0].segment_mounts(usize::from(leg.thigh)).to_vec();
    for m in &mounts {
        let [x, y, z] = m.map(usize::from);
        assert!(r.units[0].body.remove(x, y, z));
        r.units[0].removed.push(*m);
    }
    r.matter.annihilate_body(0, mounts.len());
    r.units[0].refresh_damage(0);
    let cap = r.matter.tuning.max_flying;
    r.matter.tuning.max_flying = 0;
    let original = r.units[0].body.segments[usize::from(leg.thigh)]
        .body
        .solid_count();
    for budget in [1, 19, 101, 4096] {
        r.matter.shed_segments_budget(&mut r.units, budget);
        assert_eq!(
            r.units[0].body.segments[usize::from(leg.thigh)]
                .body
                .solid_count(),
            original
        );
        let copy = BlastRange::restore(r.config, &r.snapshot()).unwrap();
        assert_eq!(r.state_hash(), copy.state_hash());
    }
    r.matter.tuning.max_flying = cap;
    let mut copy = BlastRange::restore(r.config, &r.snapshot()).unwrap();
    for _ in 0..120 {
        r.step();
        copy.step();
        assert_eq!(r.state_hash(), copy.state_hash());
    }
    assert_eq!(
        r.units[0].body.segments[usize::from(leg.thigh)]
            .body
            .solid_count(),
        0
    );
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn version_34_loads_and_new_structure_jobs_reject_truncated_data() {
    let r = scene(RangeUnitKind::Tank);
    let bytes = r.snapshot();
    let mut tail = fx::blob::Blob::new();
    r.save_after(&mut tail, 34);
    let mut old = bytes[..bytes.len() - tail.v.len()].to_vec();
    old[..4].copy_from_slice(&34u32.to_le_bytes());
    let copy = BlastRange::restore(r.config, &old).unwrap();
    assert_eq!(copy.state_hash(), r.state_hash());
    assert!(BlastRange::restore(r.config, &bytes[..bytes.len() - 1]).is_none());
}

#[test]
fn cutting_through_a_shin_releases_the_leg_as_matter() {
    let mut r = scene(RangeUnitKind::Strider);
    // Cut in mid-air so neither half can merely rest on the foot and
    // ankle that detach with it at ground level.
    r.units[0].motion.hull.y = r.units[0].motion.hull.y.add(Fx::from_int(24));
    r.units[0].motion.pos[1] = r.units[0].motion.pos[1].add(Fx::from_int(24));
    r.units[0].update_pose(0);
    let leg = r.units[0].body.legs[0];
    let seg = usize::from(leg.shin);
    let u = &mut r.units[0];
    let b = &mut u.body.segments[seg].body;
    let x = b.vol.nx / 2;
    let mut count = 0;
    for z in 0..b.vol.nz {
        for y in 0..b.vol.ny {
            if b.remove(x as usize, y as usize, z as usize) {
                u.seg_removed.push((leg.shin, [x, y, z]));
                count += 1;
            }
        }
    }
    r.matter.annihilate_body(0, count);
    r.units[0].refresh_damage(0);
    assert!(
        r.units[0].motion.legs_lost[0],
        "severed leg remains a kinematic limb"
    );
    for _ in 0..90 {
        r.step();
    }
    assert_eq!(r.units[0].body.segments[seg].body.solid_count(), 0);
    let parts: Vec<_> = r
        .matter
        .debris
        .fragments
        .iter()
        .filter(|f| {
            f.chunk
                .as_ref()
                .is_some_and(|c| c.segment == Some(leg.shin))
        })
        .collect();
    assert_eq!(
        parts.len(),
        2,
        "the two sides of the cut stayed glued together"
    );
    assert!(parts.iter().all(|f| f.resting), "severed shin never landed");
    assert!(
        parts
            .iter()
            .any(|f| f.position.y < f.chunk.as_ref().unwrap().centre.y.sub(Fx::ONE)),
        "shin pieces: {:?}",
        parts
            .iter()
            .map(|f| (f.position, f.chunk.as_ref().unwrap().centre, f.bounds()))
            .collect::<Vec<_>>()
    );
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn losing_three_legs_lowers_the_actual_geometry_and_stops_the_drive() {
    let mut r = scene(RangeUnitKind::Strider);
    for _ in 0..30 {
        r.step();
    }
    let start = r.units[0].motion.pos;
    let u = &mut r.units[0];
    let mut count = 0;
    for li in 0..3 {
        let seg = u.body.legs[li].shin;
        let b = &mut u.body.segments[usize::from(seg)].body;
        let x = b.vol.nx / 2;
        for z in 0..b.vol.nz {
            for y in 0..b.vol.ny {
                if b.remove(x as usize, y as usize, z as usize) {
                    u.seg_removed.push((seg, [x, y, z]));
                    count += 1;
                }
            }
        }
    }
    r.matter.annihilate_body(0, count);
    let u = &mut r.units[0];
    u.refresh_damage(0);
    assert!(u.status.alive && u.status.engine);
    assert_eq!(u.motion.legs_lost, [true, true, true, false]);
    u.motion.enabled = true;
    let goal = [start[0].to_int_trunc() + 80, start[2].to_int_trunc()];
    u.motion.destination = Some(goal);
    u.motion.path = vec![goal];
    for _ in 0..120 {
        r.step();
    }
    let u = &r.units[0];
    assert_eq!(
        [u.motion.pos[0], u.motion.pos[2]],
        [start[0], start[2]],
        "drove on one leg"
    );
    assert!(
        u.pose.base.y < start[1].sub(Fx::from_int(10)),
        "visual root never came down: {:?}",
        u.pose.base
    );
    let b = &u.body;
    let mut bottom = i32::MAX;
    for z in 0..b.vol.nz as usize {
        for y in 0..b.vol.ny as usize {
            for x in 0..b.vol.nx as usize {
                if b.vol.get(x, y, z) {
                    bottom = bottom.min(u.world_pos([x, y, z])[1]);
                }
            }
        }
    }
    assert!(
        (FLOOR..=FLOOR + 6).contains(&bottom),
        "belly did not land: {bottom}"
    );
    let copy = BlastRange::restore(r.config, &r.snapshot()).unwrap();
    assert_eq!(r.state_hash(), copy.state_hash());
}

#[test]
fn viaduct_loses_a_severed_half_with_rails_and_overlapping_caps() {
    let base = scene(RangeUnitKind::Tank);
    let authored = RangeLayout::from_ron(include_str!("../../../data/range-km.ron")).unwrap();
    for split in [false, true] {
        let mut layout = base.layout.clone().unwrap();
        layout.size = [512, 128, 256];
        layout.clear_min = [0, 0];
        layout.clear_max = [512, 256];
        layout.gun = [120, 16, 180];
        layout.target = [425, 42, 58];
        layout.units.clear();
        // Use the real map's deck, piers, caps and parapets, translated into a
        // small world. A gap reproduces the original independent-span report.
        layout.blocks = authored
            .blocks
            .iter()
            .filter(|b| {
                b.min[0] >= 7905
                    && b.max[0] <= 8305
                    && b.min[2] >= 3470
                    && b.max[2] <= 3540
                    && b.max[1] >= 90
            })
            .cloned()
            .map(|mut b| {
                for p in [&mut b.min, &mut b.max] {
                    p[0] -= 7865;
                    p[2] -= 3432;
                }
                b
            })
            .collect();
        if split {
            layout.blocks.push(modding::range::RangeBlock {
                min: [231, 90, 43],
                max: [239, 109, 103],
                material: 0,
            });
        }
        let mut r = BlastRange::with_layout(base.config, layout).unwrap();
        r.control();
        // Warm the support graph first: the connectivity solver can then
        // claim a wholly disconnected half before the bending pass sees it.
        r.world.queue_dig(416, 20, 52);
        for _ in 0..20 {
            r.step();
        }
        assert!(
            r.world.solid(350, 95, 60),
            "minor chip collapsed an intact pier"
        );
        for x in 415..427 {
            for z in 52..68 {
                // A disconnected, perfectly vertical pier cut by one cell
                // legitimately settles back onto its lower stump. Remove
                // that pier for the free-fall case; the continuous bridge
                // instead exercises bending failure after a thin cut.
                let cut = if split { 16..78 } else { 30..31 };
                for y in cut {
                    r.world.queue_dig(x, y, z);
                }
            }
        }
        let mut tilted = false;
        let mut fell = false;
        let mut replay: Option<BlastRange> = None;
        for step in 0..180 {
            r.step();
            if r.bodies
                .list
                .iter()
                .any(|b| b.tilt_axis != 0 && b.awaiting_removal(&r.world))
            {
                assert!(
                    r.bodies.list.len() <= 2,
                    "source matter detached twice (split={split})"
                );
            }
            if let Some(copy) = &mut replay {
                copy.step();
                assert_eq!(
                    r.state_hash(),
                    copy.state_hash(),
                    "pending fracture replay at {step}"
                );
            }
            if step == 1 {
                replay = Some(BlastRange::restore(r.config, &r.snapshot()).unwrap());
            }
            if step == 8 {
                replay = None;
            }
            tilted |= r.bodies.list.iter().any(|b| b.tilt_angle.abs() > 100);
            fell |= r.bodies.list.iter().any(|b| b.fallen > 0);
        }
        assert!(
            fell && (split || tilted),
            "viaduct did not fall and tip (split={split})"
        );
        assert!(
            !r.world.solid(350, 95, 60),
            "deck still hangs (split={split})"
        );
        assert!(
            !r.world.solid(350, 104, 100),
            "parapet still hangs (split={split})"
        );
        assert!(
            r.world.solid(120, 95, 60),
            "supported half disappeared (split={split})"
        );
        let mut replay = BlastRange::restore(r.config, &r.snapshot()).unwrap();
        for _ in 0..6 {
            r.step();
            replay.step();
            assert_eq!(r.state_hash(), replay.state_hash());
        }
    }
}

#[test]
fn unsupported_concrete_outlives_neither_a_small_particle_pool_nor_a_partial_edit_drain() {
    let base = scene(RangeUnitKind::Tank);
    let mut layout = base.layout.unwrap();
    layout.size = [256, 128, 256];
    layout.clear_min = [0, 0];
    layout.clear_max = [256, 256];
    layout.gun = [64, 16, 64];
    layout.target = [180, 24, 128];
    layout.units[0].at = [180, 16, 64];
    layout.blocks = vec![
        modding::range::RangeBlock {
            min: [128, 16, 128],
            max: [132, 52, 132],
            material: 2,
        },
        modding::range::RangeBlock {
            min: [100, 52, 100],
            max: [160, 56, 160],
            material: 2,
        },
    ];
    let mut r = BlastRange::with_layout(base.config, layout).unwrap();
    r.control();
    r.matter.tuning.max_fragments = 8;
    r.collapse.max_promote_per_tick = 1;
    r.collapse.max_relabel_per_tick = 1;
    assert!(r.world.solid(108, 53, 108), "fixture has no roof");
    for x in 128..132 {
        for z in 128..132 {
            for y in 16..52 {
                r.world.queue_dig(x, y, z);
            }
        }
    }
    let mut falling = false;
    for _ in 0..240 {
        r.step();
        falling |= r.bodies.list.iter().any(|b| b.fallen > 0);
    }
    assert!(
        falling,
        "unsupported slab never fell: collapsed={} bodies={} pending={}",
        r.collapsed_cells,
        r.bodies.list.len(),
        r.collapse.pending()
    );
    assert!(
        !r.world.solid(108, 53, 108),
        "roof stayed at its original altitude"
    );
    assert!(
        r.world.solid(108, 19, 108),
        "fallen roof did not settle on the ground"
    );
}
