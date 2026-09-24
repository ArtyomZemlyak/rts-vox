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
        let mut a = BlastRange::with_layout(c.clone(), layout.clone()).unwrap();
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
        let mut b = BlastRange::restore(c.clone(), &a.snapshot()).unwrap();
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
    let c = ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
    let mut layout = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
    layout.course = None;
    layout.blocks.clear();
    layout.clear_min = [64, 64];
    layout.clear_max = [600, 600];
    layout.units = [300, 361]
        .map(|x| modding::range::RangeUnitDef {
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
        let mut a = BlastRange::with_layout(c.clone(), layout.clone()).unwrap();
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
        let mut b = BlastRange::restore(c.clone(), &a.snapshot()).unwrap();
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
            allied: false,
            kind: RangeUnitKind::Tank,
            at: [x, FLOOR, 336],
        })
        .to_vec();
    let mut push = Vec::new();
    for moving_target in [false, true] {
        let mut a = BlastRange::with_layout(c.clone(), layout.clone()).unwrap();
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
        let mut replay = BlastRange::restore(c.clone(), &a.snapshot()).unwrap();
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
        let mut r = BlastRange::with_layout(c.clone(), layout).unwrap();
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
    let mut restored = BlastRange::restore(r.config.clone(), &r.snapshot()).unwrap();
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
        let copy = BlastRange::restore(r.config.clone(), &r.snapshot()).unwrap();
        assert_eq!(r.state_hash(), copy.state_hash());
    }
    r.matter.tuning.max_flying = cap;
    let mut copy = BlastRange::restore(r.config.clone(), &r.snapshot()).unwrap();
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
    r.save_structure(&mut tail);
    r.battle.save(&mut tail);
    let mut old = bytes[..bytes.len() - tail.v.len()].to_vec();
    old[..4].copy_from_slice(&34u32.to_le_bytes());
    let copy = BlastRange::restore(r.config.clone(), &old).unwrap();
    assert_eq!(copy.state_hash(), r.state_hash());
    assert!(BlastRange::restore(r.config.clone(), &bytes[..bytes.len() - 1]).is_none());
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
    let copy = BlastRange::restore(r.config.clone(), &r.snapshot()).unwrap();
    assert_eq!(r.state_hash(), copy.state_hash());
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
