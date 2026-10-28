//! Long-order fallback must plan a useful local leg before sweeping kilometres.
use super::*;
use modding::range::RangeUnitKind;

struct Grove;
impl voxel::Generator for Grove {
    fn material(&self, x: u32, y: u32, _: u32) -> u16 {
        if y < 16 {
            2
        } else if ((400..406).contains(&x) || (800..806).contains(&x)) && y < 40 {
            4
        } else {
            0
        }
    }
}

fn step(m: &mut Motion, world: &World, body: &UnitBody, tick: u32) {
    m.allow_resume(m.navigation.search_resume_per_unit);
    m.step(
        world,
        body,
        true,
        true,
        crate::mobility::Mobility::intact(true),
        &[],
        tick,
        98,
        &nav::vehicle::Rubble::new(),
        &Default::default(),
    );
}

#[test]
fn a_distant_off_map_order_cannot_become_an_admitted_local_leg() {
    let world = World::new(1024, 128, 1024, 65536, Box::new(Grove));
    for (kind, body) in [
        (RangeUnitKind::Tank, voxel::unit::tank()),
        (RangeUnitKind::Artillery, voxel::unit::artillery()),
        (RangeUnitKind::Strider, voxel::unit::strider()),
        (RangeUnitKind::Spider, voxel::unit::spider()),
    ] {
        for goal in [
            [8000, 500],
            [500, 8000],
            [-8000, 500],
            [500, -8000],
            [8000, 8000],
        ] {
            for unwalked in [false, true] {
                let mut m = Motion::new([500, 16, 500], kind, &body);
                m.defer_fine(unwalked);
                assert!(
                    !m.move_to(&world, goal, &body, &[], &Default::default()),
                    "{kind:?} admitted {goal:?}"
                );
                assert!(m.destination.is_none() && m.path.is_empty() && !m.searching());
            }
        }
    }
}

#[test]
fn distant_local_fallback_moves_early_crosses_trees_and_reaches_the_real_order() {
    for (kind, body) in [
        (RangeUnitKind::Tank, voxel::unit::tank()),
        (RangeUnitKind::Artillery, voxel::unit::artillery()),
        (RangeUnitKind::Strider, voxel::unit::strider()),
        (RangeUnitKind::Spider, voxel::unit::spider()),
    ] {
        let world = World::new(2048, 128, 1024, 65536, Box::new(Grove));
        world.prepare_navigation([4, 5]);
        let goal = [1800, 500];
        let mut m = Motion::new([200, 16, 500], kind, &body);
        m.set_global_search_limit(0);
        let mut legacy = m.clone();
        legacy.navigation.global_min_distance = MAX_SIDE;
        for actor in [&mut m, &mut legacy] {
            actor.defer_fine(true);
            assert!(actor.move_to(&world, goal, &body, &[], &Default::default()));
            actor.defer_fine(false);
        }
        let start = m.pos;
        let mut first = [None; 2];
        let mut next_tick = 0;
        for tick in 0..128 {
            next_tick = tick + 1;
            for (k, actor) in [&mut m, &mut legacy].into_iter().enumerate() {
                step(actor, &world, &body, tick);
                if first[k].is_none() && actor.pos[0] != start[0] {
                    first[k] = Some(tick + 1);
                }
            }
            if first.iter().all(Option::is_some) {
                break;
            }
        }
        println!(
            "{kind:?} first_motion_ticks local={:?} full={:?}",
            first[0], first[1]
        );
        assert!(first[0].is_some_and(|n| n <= 12), "{kind:?}: {first:?}");
        assert!(first[0].unwrap() < first[1].unwrap(), "{kind:?}: {first:?}");
        assert_eq!(m.destination, Some(goal));
        assert!(m.partial_path);
        let mut cut = false;
        for tick in next_tick..2600 {
            step(&mut m, &world, &body, tick);
            assert!(m.drive.vel <= Fx::from_int(m.speed), "speed cap changed");
            cut |= !m.cut.is_empty();
            if m.destination.is_none() {
                break;
            }
        }
        assert!(cut, "{kind:?}: forest was not crossed");
        assert!(m.destination.is_none(), "{kind:?}: stopped at {:?}", m.pos);
        assert!((m.pos[0].to_int_trunc() - goal[0]).abs() <= m.arrival + 1);
    }
}

#[test]
fn a_checked_global_prefix_moves_while_the_search_and_its_snapshot_continue() {
    let world = World::new(4096, 128, 1024, 65536, Box::new(Grove));
    world.prepare_navigation([4, 5]);
    let body = voxel::unit::spider();
    let mut m = Motion::new([200, 16, 500], RangeUnitKind::Spider, &body);
    let rubble = nav::vehicle::Rubble::new();
    m.allow_coarse(Some(0));
    assert!(m.move_to(&world, [3800, 500], &body, &[], &rubble));
    m.allow_coarse(None);
    let mut tick = 1;
    while m.path.is_empty() {
        let spent = m.continue_coarse(
            &world,
            &body,
            Vec::new,
            &rubble,
            tick,
            &Default::default(),
            4000,
            true,
            false,
        );
        assert!(spent <= 4000);
        tick += 1;
        assert!(tick < 20);
    }
    assert!(m.partial_path && m.coarse.is_some());
    let mut bytes = Blob::new();
    m.save(&mut bytes);
    m.save_coarse(&mut bytes);
    let mut restored = Motion::new([200, 16, 500], RangeUnitKind::Spider, &body);
    let mut scan = Scan::new(&bytes.v);
    assert!(restored.load(&mut scan));
    assert!(restored.load_coarse(&mut scan, [4096, 1024]));
    let start = m.pos[0];
    let mut moved_pending = false;
    for t in tick..tick + 40 {
        for actor in [&mut m, &mut restored] {
            let spent = actor.continue_coarse(
                &world,
                &body,
                Vec::new,
                &rubble,
                t,
                &Default::default(),
                4000,
                true,
                false,
            );
            assert!(spent <= 4000);
            actor.allow_start(false);
            step(actor, &world, &body, t);
        }
        moved_pending |= m.pos[0] > start && m.coarse.is_some();
        let (mut left, mut right) = (Blob::new(), Blob::new());
        for (actor, out) in [(&m, &mut left), (&restored, &mut right)] {
            actor.save(out);
            if actor.coarse.is_some() {
                actor.save_coarse(out);
            }
        }
        assert_eq!(left.v, right.v, "pending preview diverged at {t}");
    }
    assert!(
        moved_pending,
        "the hull still waited for the full global answer"
    );
    assert_eq!(m.destination, Some([3800, 500]));
}

#[test]
fn a_local_leg_search_restores_and_short_orders_keep_their_original_target() {
    let world = World::new(2048, 128, 1024, 65536, Box::new(Grove));
    world.prepare_navigation([4, 5]);
    let body = voxel::unit::spider();
    let mut m = Motion::new([200, 16, 500], RangeUnitKind::Spider, &body);
    m.set_global_search_limit(0);
    assert_eq!(m.local_goal([500, 500]), [500, 500]);
    for goal in [[1800, 500], [1800, 900], [60, 900]] {
        let local = m.local_goal(goal);
        assert!((0..2).all(|a| (EDGE..2048 - EDGE).contains(&local[a])));
        assert!((0..2).any(|a| (goal[a] - local[a]).abs() > 8 * m.navigation.cell_units));
    }
    m.defer_fine(true);
    assert!(m.move_to(&world, [1800, 500], &body, &[], &Default::default()));
    m.defer_fine(false);
    step(&mut m, &world, &body, 0);
    assert!(m.searching());
    let mut saved = Blob::new();
    m.save(&mut saved);
    let mut restored = Motion::new([200, 16, 500], RangeUnitKind::Spider, &body);
    restored.set_global_search_limit(0);
    assert!(restored.load(&mut Scan::new(&saved.v)));
    for tick in 1..96 {
        step(&mut m, &world, &body, tick);
        step(&mut restored, &world, &body, tick);
        let (mut a, mut b) = (Blob::new(), Blob::new());
        m.save(&mut a);
        restored.save(&mut b);
        assert_eq!(a.v, b.v, "restored local leg diverged at {tick}");
    }
}
