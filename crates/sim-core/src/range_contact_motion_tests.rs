//! Contact manoeuvres use current geometry even while routing has no budget.
use super::*;
use modding::range::RangeUnitKind;

struct Flat;
impl voxel::Generator for Flat {
    fn material(&self, _: u32, y: u32, _: u32) -> u16 {
        u16::from(y < 16)
    }
}
fn world() -> World {
    let w = World::new(1536, 384, 1536, 65536, Box::new(Flat));
    w.prepare_navigation([4, 5]);
    w
}
fn step(m: &mut Motion, w: &World, b: &UnitBody, others: &[Obstacle], tick: u32) {
    m.allow_start(false);
    m.step(
        w,
        b,
        true,
        true,
        crate::mobility::Mobility::intact(true),
        others,
        tick,
        98,
        &nav::vehicle::Rubble::new(),
        &Default::default(),
    );
}

#[test]
fn rolling_tracks_keep_clear_translation_when_the_turn_clips_a_wall() {
    let w = world();
    for kind in [RangeUnitKind::Tank, RangeUnitKind::Crawler] {
        let unit = super::super::RangeUnit::of_kind(kind, [600, 16, 600]);
        let mut m = unit.motion;
        let hull = Obstacle::of(&m, &unit.body);
        let wall = Obstacle {
            height: Fx::ZERO,
            moving: false,
            head: 0,
            pos: [
                Fx::from_int(600),
                Fx::from_int(16),
                hull.pos[2]
                    .add(hull.half[1])
                    .add(Fx::from_int(30))
                    .sub(Fx::from_int(3)),
            ],
            half: [Fx::from_int(50), Fx::from_int(30)],
        };
        m.queue_move(&w, [1200, 680]);
        m.drive.vel = Fx::from_int(m.speed / 2);
        let mut control = m.clone();
        let before = m.pos;
        for tick in 0..36 {
            step(&mut m, &w, &unit.body, &[wall], tick);
            step(&mut control, &w, &unit.body, &[], tick);
            assert!(Obstacle::of(&m, &unit.body).penetration(&wall) <= HULL_CONTACT_SLOP);
        }
        println!(
            "{kind:?}: contact {:?}, open {:?}, speed {}",
            m.pos[0].sub(before[0]),
            control.pos[0].sub(before[0]),
            m.speed
        );
        assert!(
            m.pos[0].sub(before[0]) > control.pos[0].sub(before[0]).div(Fx::from_int(2)),
            "{kind:?}: rolling hull lost a clear forward lane: {:?}, blocked {}",
            m.pos,
            m.blocked
        );
        let mut replay = m.clone();
        assert!(replay.load(&mut Scan::new(&bytes(&m))));
        for tick in 36..1200 {
            step(&mut m, &w, &unit.body, &[wall], tick);
            step(&mut replay, &w, &unit.body, &[wall], tick);
            assert_eq!(
                bytes(&m),
                bytes(&replay),
                "{kind:?}: restored egress diverged"
            );
            assert!(Obstacle::of(&m, &unit.body).penetration(&wall) <= HULL_CONTACT_SLOP);
            if m.destination.is_none() {
                break;
            }
        }
        assert!(
            m.destination.is_none(),
            "{kind:?}: wall-following did not finish {:?}",
            m.pos
        );
    }
}

fn bytes(m: &Motion) -> Vec<u8> {
    let mut b = Blob::new();
    m.save(&mut b);
    b.v
}

#[test]
fn grazing_contacts_work_on_both_sides_and_rotated_walls() {
    let w = world();
    for kind in [RangeUnitKind::Tank, RangeUnitKind::Crawler] {
        for heading in [0, angle::FULL / 8, angle::FULL / 4] {
            for side in [-1, 1] {
                let unit = super::super::RangeUnit::of_kind(kind, [600, 16, 600]);
                let mut m = unit.motion;
                m.drive.head = heading;
                let hull = Obstacle::of(&m, &unit.body);
                let forward = [angle::cos(heading), angle::sin(heading)];
                let normal = [forward[1].neg(), forward[0]];
                let offset = hull.half[1].add(Fx::from_int(27)).mul(Fx::from_int(side));
                let wall = Obstacle {
                    height: Fx::ZERO,
                    moving: false,
                    head: heading,
                    pos: [
                        m.pos[0].add(normal[0].mul(offset)),
                        m.pos[1],
                        m.pos[2].add(normal[1].mul(offset)),
                    ],
                    half: [Fx::from_int(50), Fx::from_int(30)],
                };
                let goal = [0, 1].map(|a| {
                    Fx::from_int(600)
                        .add(forward[a].mul(Fx::from_int(600)))
                        .add(normal[a].mul(Fx::from_int(80 * side)))
                        .to_int_trunc()
                });
                m.queue_move(&w, goal);
                m.drive.vel = Fx::from_int(m.speed / 2);
                let mut control = m.clone();
                let before = m.pos;
                for tick in 0..60 {
                    step(&mut m, &w, &unit.body, &[wall], tick);
                    step(&mut control, &w, &unit.body, &[], tick);
                    assert!(Obstacle::of(&m, &unit.body).penetration(&wall) <= HULL_CONTACT_SLOP);
                    assert!(m.drive.vel.abs() <= Fx::from_int(m.speed));
                }
                let advance = |p: [Fx; 3]| {
                    p[0].sub(before[0])
                        .mul(forward[0])
                        .add(p[2].sub(before[2]).mul(forward[1]))
                };
                assert!(
                    advance(m.pos) > advance(control.pos).div(Fx::from_int(2)),
                    "{kind:?} head {heading} side {side}: {:?}, open {:?}",
                    advance(m.pos),
                    advance(control.pos)
                );
            }
        }
    }
}

#[test]
fn bipeds_finish_a_grazing_wall_order_and_resume_a_saved_tangent() {
    let w = world();
    for kind in [RangeUnitKind::Commander, RangeUnitKind::Walker04] {
        let unit = super::super::RangeUnit::of_kind(kind, [600, 16, 600]);
        let mut m = unit.motion;
        let hull = Obstacle::of(&m, &unit.body);
        let wall = Obstacle {
            height: Fx::ZERO,
            moving: false,
            head: 0,
            pos: [
                Fx::from_int(600),
                Fx::from_int(16),
                hull.pos[2]
                    .add(hull.half[1])
                    .add(Fx::from_int(30))
                    .sub(Fx::from_int(3)),
            ],
            half: [Fx::from_int(50), Fx::from_int(30)],
        };
        m.queue_move(&w, [1050, 690]);
        let mut replay = None;
        let mut prefix = false;
        let mut blocked = 0;
        for tick in 0..1800 {
            step(&mut m, &w, &unit.body, &[wall], tick);
            blocked += u32::from(m.blocked);
            assert!(Obstacle::of(&m, &unit.body).penetration(&wall) <= HULL_CONTACT_SLOP);
            assert!(m.drive.vel.abs() <= Fx::from_int(m.speed));
            if let Some(saved) = &mut replay {
                step(saved, &w, &unit.body, &[wall], tick);
                assert_eq!(bytes(&m), bytes(saved), "{kind:?} replay at {tick}");
            } else if m.partial_path && !m.path.is_empty() {
                let mut saved = m.clone();
                assert!(saved.load(&mut Scan::new(&bytes(&m))));
                replay = Some(saved);
                prefix = true;
            }
            if m.destination.is_none() {
                break;
            }
        }
        assert!(prefix, "{kind:?}: fixture missed the tangent manoeuvre");
        assert!(
            m.destination.is_none(),
            "{kind:?} failed to finish: {:?} {:?}",
            m.pos,
            m.path
        );
        assert!(
            blocked <= 4,
            "{kind:?}: {blocked} fully refused ticks at a grazing contact"
        );
    }
}

#[test]
fn a_contact_tangent_keeps_live_neighbours_and_new_terrain_solid() {
    let mut w = world();
    let unit = super::super::RangeUnit::of_kind(RangeUnitKind::Tank, [600, 16, 600]);
    let mut m = unit.motion;
    let hull = Obstacle::of(&m, &unit.body);
    let side = Obstacle {
        height: Fx::ZERO,
        moving: false,
        head: 0,
        pos: [
            Fx::from_int(600),
            Fx::from_int(16),
            hull.pos[2]
                .add(hull.half[1])
                .add(Fx::from_int(30))
                .sub(Fx::from_int(3)),
        ],
        half: [Fx::from_int(50), Fx::from_int(30)],
    };
    m.queue_move(&w, [1200, 680]);
    m.drive.vel = Fx::from_int(m.speed / 2);
    let mut tick = 0;
    while !m.partial_path && tick < 30 {
        step(&mut m, &w, &unit.body, &[side], tick);
        tick += 1;
    }
    assert!(m.partial_path, "fixture never selected a local tangent");
    let mut front = hull;
    front.half = [Fx::from_int(8); 2];
    front.pos[0] = m.pos[0].add(hull.half[0]).add(front.half[0]);
    front.pos[2] = m.pos[2];
    let stop = m.pos[0].add(HULL_CONTACT_SLOP);
    for tick in tick..tick + 50 {
        step(&mut m, &w, &unit.body, &[side, front], tick);
        assert!(Obstacle::of(&m, &unit.body).penetration(&front) <= HULL_CONTACT_SLOP);
    }
    assert!(
        m.pos[0] <= stop.add(Fx::ONE),
        "local steering passed through a second neighbour"
    );
    // The same local prefix cannot keep a stale answer when a wall is edited
    // into its clear lane. Build only the few columns the next hull spans.
    let x = m.pos[0].to_int_trunc() + m.shape(&unit.body).half[0] + 2;
    for z in 540..660 {
        for y in 16..80 {
            w.queue_edit(voxel::Edit::put(x as u32, y, z, 2));
        }
    }
    while w.queued() > 0 {
        w.flush();
    }
    for z in 540..660 {
        assert_eq!(w.material(x as u32, 40, z), 2);
    }
    let at = m.pos;
    for tick in 100..140 {
        m.allow_start(false);
        m.step(
            &w,
            &unit.body,
            true,
            true,
            crate::mobility::Mobility::intact(true),
            &[side],
            tick,
            98,
            &nav::vehicle::Rubble::new(),
            &if tick == 100 {
                nav::vehicle::Changed::everywhere()
            } else {
                Default::default()
            },
        );
    }
    assert!(
        m.pos[0] < Fx::from_int(x),
        "local tangent ignored changed terrain"
    );
    let stopped = m.pos;
    for tick in 140..200 {
        step(&mut m, &w, &unit.body, &[side], tick);
    }
    assert!(
        m.pos[0].sub(stopped[0]).abs() < Fx::ONE,
        "new wall did not stop the local prefix: before {at:?}, after {:?}, wall {x}",
        m.pos
    );
}
