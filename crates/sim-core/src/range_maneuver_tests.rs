use super::*;
use modding::range::RangeUnitKind;

struct Flat;
impl voxel::Generator for Flat {
    fn material(&self, _: u32, y: u32, _: u32) -> u16 {
        u16::from(y < 16)
    }
}

#[test]
fn adjacent_parked_hulls_get_a_bounded_local_cluster_bypass() {
    let w = World::new(2000, 96, 2000, 65536, Box::new(Flat));
    w.prepare_navigation([4, 5]);
    for kind in [
        RangeUnitKind::Tank,
        RangeUnitKind::Commander,
        RangeUnitKind::Crawler,
        RangeUnitKind::Walker04,
        RangeUnitKind::Spider,
    ] {
        let u = super::super::RangeUnit::of_kind(kind, [300, 16, 800]);
        let mut m = u.motion;
        let body = u.body;
        let own = Obstacle::of(&m, &body);
        let radius = if m.leg_side_guard {
            own.half[0]
        } else {
            own.half[0]
                .mul(own.half[0])
                .add(own.half[1].mul(own.half[1]))
                .sqrt()
        };
        m.pos[0] = Fx::from_int(800 - 24 - 40).sub(radius);
        let spacing = 40 + own.half[1].to_int_trunc();
        let walls: Vec<_> = [-1, 0, 1]
            .into_iter()
            .map(|i| Obstacle {
                moving: false,
                pos: [800, 16, 800 + i * spacing].map(Fx::from_int),
                height: Fx::ZERO,
                head: 0,
                half: [24, 20].map(Fx::from_int),
            })
            .collect();
        let goal = [1150, 800];
        assert!(m.queue_move(&w, goal));
        assert!(
            m.flat_hull_detour(&w, goal, &body, &walls, &nav::vehicle::Rubble::new()),
            "{kind:?}: neighbouring hulls forced the slow oriented search"
        );
        assert!(!m.searching());
        assert!(!m.path.is_empty());
        assert!(m.path.iter().any(|p| (p[1] - 800).abs() > spacing));
        let mut arrived = None;
        for tick in 0..3600 {
            m.allow_start(false);
            m.step(
                &w,
                &body,
                true,
                true,
                crate::mobility::Mobility::intact(true),
                &walls,
                tick,
                98,
                &nav::vehicle::Rubble::new(),
                &Default::default(),
            );
            assert!(!m.blocked,"{kind:?}: rejected cluster step {tick}, pos={:?}, drive={:?}, path={:?}, cursor={}, ground={:?}",m.pos,m.drive,m.path,m.cursor,m.ground_memo);
            let own = Obstacle::of(&m, &body);
            for wall in &walls {
                assert!(
                    own.penetration(wall) <= HULL_CONTACT_SLOP,
                    "{kind:?} entered wall at {tick}"
                );
            }
            assert!(m.drive.vel.abs() <= Fx::from_int(m.speed));
            if m.destination.is_none() {
                arrived = Some(tick + 1);
                break;
            }
        }
        assert!(
            arrived.is_some(),
            "{kind:?}: cluster route never arrived: {:?}, {:?}",
            m.pos,
            m.path
        );
        eprintln!("cluster {kind:?} arrival={}", arrived.unwrap());
    }
}

#[test]
fn dense_cluster_falls_back_without_losing_the_order() {
    let w = World::new(1200, 96, 1200, 65536, Box::new(Flat));
    w.prepare_navigation([4, 5]);
    let u = super::super::RangeUnit::of_kind(RangeUnitKind::Commander, [350, 16, 500]);
    let body = u.body;
    let mut m = u.motion;
    let walls: Vec<_> = (0..10)
        .map(|i| Obstacle {
            moving: false,
            height: Fx::ZERO,
            head: 0,
            pos: [500, 16, 365 + i * 30].map(Fx::from_int),
            half: [30, 20].map(Fx::from_int),
        })
        .collect();
    let goal = [850, 500];
    m.queue_move(&w, goal);
    assert!(!m.flat_hull_detour(&w, goal, &body, &walls, &nav::vehicle::Rubble::new()));
    assert_eq!(m.destination, Some(goal));
}

#[test]
fn large_biped_yard_proof_includes_rotated_terrain_probes_at_map_edge() {
    let w = World::new(1200, 96, 1200, 65536, Box::new(Flat));
    w.prepare_navigation([4, 5]);
    let u = super::super::RangeUnit::of_kind(RangeUnitKind::Walker04, [236, 16, 500]);
    let body = u.body;
    let mut m = u.motion;
    let walls: Vec<_> = [-1, 0, 1]
        .into_iter()
        .map(|i| Obstacle {
            moving: false,
            height: Fx::ZERO,
            head: 0,
            pos: [500, 16, 500 + i * 240].map(Fx::from_int),
            half: [24, 20].map(Fx::from_int),
        })
        .collect();
    m.queue_move(&w, [850, 500]);
    assert!(
        !m.flat_hull_detour(&w, [850, 500], &body, &walls, &nav::vehicle::Rubble::new()),
        "yawed terrain samples extend beyond the world"
    );
    assert_eq!(m.destination, Some([850, 500]));
}

#[test]
fn divergent_local_waypoints_keep_biped_passing_in_the_command_frame() {
    let kinds = [RangeUnitKind::Commander, RangeUnitKind::Walker04];
    let starts = [[600, 16, 747], [838, 16, 708]];
    let goals = [[800, 1350], [800, 350]];
    let mut units: Vec<_> = kinds
        .into_iter()
        .zip(starts)
        .map(|(k, p)| super::super::RangeUnit::of_kind(k, p))
        .collect();
    for (id, u) in units.iter_mut().enumerate() {
        u.motion.enabled = true;
        u.motion.destination = Some(goals[id]);
        u.motion.path = vec![if id == 0 { [619, 745] } else { [822, 710] }];
        u.motion.drive.vel = Fx::from_int(20);
    }
    let traffic: Vec<_> = units
        .iter()
        .map(|u| Traffic::of(&u.motion, &u.body, true))
        .collect();
    let hints: Vec<_> = units
        .iter()
        .enumerate()
        .map(|(id, u)| {
            u.motion
                .traffic_hint(
                    &u.body,
                    id,
                    &traffic,
                    crate::mobility::Mobility::intact(true),
                )
                .unwrap()
                .way
                .unwrap()
        })
        .collect();
    assert!(
        angle::cos(hints[0]) < Fx::ZERO,
        "commander must pass left of the larger hull"
    );
    assert!(
        angle::cos(hints[1]) > Fx::ZERO,
        "walker must maintain its other side"
    );
}

#[test]
fn authored_yard_has_four_controllable_movers_and_clear_initial_hulls() {
    let layout =
        modding::range::RangeLayout::from_ron(include_str!("../../../data/range-motion-yard.ron"))
            .unwrap();
    let orders: Vec<_> = layout
        .course
        .as_ref()
        .unwrap()
        .patrols
        .iter()
        .map(|p| (p.unit, p.points[0]))
        .collect();
    let cfg =
        modding::explosion::ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron"))
            .unwrap();
    let mut s = super::super::BlastRange::with_layout(cfg, layout).unwrap();
    s.set_physics_workers(1);
    s.set_navigation_workers(1);
    s.start_free_control().unwrap();
    assert_eq!(orders.len(), 4);
    assert_eq!(s.battle_group_move(&orders, [1300, 350]), 4);
    for a in 0..s.units.len() {
        for b in a + 1..s.units.len() {
            assert!(
                Obstacle::of_unit(&s.units[a]).penetration(&Obstacle::of_unit(&s.units[b]))
                    <= HULL_CONTACT_SLOP
            );
        }
    }
}

#[test]
fn offset_bipeds_keep_clear_opposing_lanes_without_extra_sidestep() {
    let mut units: Vec<_> = [RangeUnitKind::Commander, RangeUnitKind::Walker04]
        .into_iter()
        .map(|k| super::super::RangeUnit::of_kind(k, [900, 16, 900]))
        .collect();
    let hulls: Vec<_> = units.iter().map(Obstacle::of_unit).collect();
    let gap = hulls.iter().map(|o| o.half[1].to_int_trunc()).sum::<i32>()
        + units[0].motion.navigation.plan_margin + 4;
    for (id, u) in units.iter_mut().enumerate() {
        let m = &mut u.motion;
        m.pos[0] = Fx::from_int(if id == 0 { 600 } else { 1050 });
        m.pos[2] = Fx::from_int(900 + id as i32 * gap);
        let goal = [if id == 0 { 1500 } else { 300 }, m.pos[2].to_int_trunc()];
        m.enabled = true;
        m.destination = Some(goal);
        m.path = vec![goal];
        m.drive.head = if id == 0 { 0 } else { angle::HALF };
        m.drive.vel = Fx::from_int(20);
    }
    let traffic: Vec<_> = units.iter().map(|u| Traffic::of(&u.motion, &u.body, true)).collect();
    for (id, u) in units.iter().enumerate() {
        let hint = u.motion.traffic_hint(&u.body, id, &traffic,
            crate::mobility::Mobility::intact(true));
        assert!(hint.is_none_or(|h| h.way.is_none()),
            "clear opposing lane caused unnecessary sidestep for {:?}", u.kind);
    }
}

#[test]
fn visible_route_repair_keeps_live_obstacles_rubble_and_fresh_craters() {
    let mut w = World::new(2000, 512, 2000, 65536, Box::new(Flat));
    w.prepare_navigation([4, 5]);
    let u = super::super::RangeUnit::of_kind(RangeUnitKind::Commander, [700, 16, 800]);
    let body = u.body;
    let mut m = u.motion;
    m.enabled = true;
    m.destination = Some([1000, 800]);
    m.path = vec![[650, 600], [1000, 600], [1000, 800]];
    m.drive.vel = Fx::from_int(20);
    let before = m.clone();
    let mut rubble = nav::vehicle::Rubble::new();
    assert!(m.shorten_visible_route(&w, &body, &[], &rubble));
    assert_eq!(m.path, vec![[1000, 800]]);
    assert_eq!(m.destination, before.destination);
    assert_eq!(m.drive.vel, before.drive.vel);
    assert_eq!(m.drive.head, before.drive.head);
    for moving in [false, true] {
        m = before.clone();
        let wall = Obstacle { moving, height: Fx::ZERO, head: angle::QUARTER / 2,
            pos: [850, 16, 800].map(Fx::from_int), half: [30, 20].map(Fx::from_int) };
        assert!(!m.shorten_visible_route(&w, &body, &[wall], &rubble));
        assert_eq!(m.path, before.path, "lost usable route on failed proof");
    }
    m = before.clone();
    rubble.insert([850, 800], 20);
    assert!(!m.shorten_visible_route(&w, &body, &[], &rubble));
    assert_eq!(m.path, before.path);
    rubble.remove(&[850, 800]);
    w.queue_edit(voxel::Edit::dig(850, 15, 800));
    w.flush();
    assert!(!m.shorten_visible_route(&w, &body, &[], &rubble),
        "same-tick crater was bypassed with a stale terrain certificate");
    assert_eq!(m.path, before.path);
    w.prepare_navigation([4, 5]);
    assert!(!m.shorten_visible_route(&w, &body, &[], &rubble));
}

#[test]
fn visible_route_repair_is_local_and_preserves_a_pending_order() {
    let w = World::new(2000, 512, 2000, 65536, Box::new(Flat));
    w.prepare_navigation([4, 5]);
    let u = super::super::RangeUnit::of_kind(RangeUnitKind::Commander, [700, 16, 800]);
    let body = u.body;
    let mut m = u.motion;
    m.enabled = true;
    m.destination = Some([1800, 800]);
    m.path = vec![[750, 600], [1800, 800]];
    let path = m.path.clone();
    assert!(!m.shorten_visible_route(&w, &body, &[], &nav::vehicle::Rubble::new()));
    assert_eq!(m.path, path);
    m.destination = Some([1000, 800]);
    m.needs_recovery = true;
    assert!(!m.shorten_visible_route(&w, &body, &[], &nav::vehicle::Rubble::new()));
    assert_eq!(m.path, path);
    assert_eq!(m.destination, Some([1000, 800]));
}

#[test]
fn tank_touching_a_large_factory_can_back_off_and_resume_its_order() {
    let w = World::new(2400, 512, 2400, 65536, Box::new(Flat));
    w.prepare_navigation([4, 5]);
    let u = super::super::RangeUnit::of_kind(RangeUnitKind::Tank, [1003, 16, 650]);
    let factory = super::super::RangeUnit::of_kind(RangeUnitKind::KilFactoryT1, [1350, 16, 500]);
    let wall = Obstacle::of_unit(&factory);
    let body = u.body;
    let mut m = u.motion;
    m.pos[0] = Fx::from_ratio(1003857849, 1000000);
    m.queue_move(&w, [1800, 650]);
    let start = m.pos;
    let mut moved = false;
    for tick in 0..2000 {
        m.allow_start(true);
        m.step(&w, &body, true, true, crate::mobility::Mobility::intact(true),
            &[wall], tick, 98, &nav::vehicle::Rubble::new(), &Default::default());
        moved |= m.pos[0].sub(start[0]).abs() > Fx::from_int(8)
            || m.pos[2].sub(start[2]).abs() > Fx::from_int(8);
        assert!(Obstacle::of(&m, &body).penetration(&wall) <= HULL_CONTACT_SLOP);
        if m.destination.is_none() { break; }
    }
    assert!(moved, "factory-side stall: pos={:?} path={:?} recovery={:?} fine={:?} coarse={:?}",
        m.pos,m.path,m.recovery_state(),m.fine_work(),m.coarse_waited());
    assert!(m.destination.is_none(), "factory route did not finish: {:?} {:?}",m.pos,m.path);
}

#[test]
fn commander_finishes_a_factory_bypass_with_a_parked_tank_near_the_exit() {
    let w = World::new(2400, 512, 2400, 65536, Box::new(Flat));
    w.prepare_navigation([4, 5]);
    let u = super::super::RangeUnit::of_kind(RangeUnitKind::Commander, [450, 16, 700]);
    let walls: Vec<_> = [(RangeUnitKind::KilFactoryT1,[1350,16,500]),
        (RangeUnitKind::Base,[1000,16,1000]),(RangeUnitKind::GrebenStorage,[1000,16,1400]),
        (RangeUnitKind::Tank,[1800,16,850])].into_iter().map(|(k,p)|
            Obstacle::of_unit(&super::super::RangeUnit::of_kind(k,p))).collect();
    let body = u.body;
    let mut m = u.motion;
    m.queue_move(&w, [1800,700]);
    for tick in 0..2500 {
        m.allow_start(true);
        m.step(&w, &body, true, true, crate::mobility::Mobility::intact(true),
            &walls, tick, 98, &nav::vehicle::Rubble::new(), &Default::default());
        if m.blocked {
            let contact = m.take_contact().map(|(id,_)|id);
            eprintln!("factory commander blocked tick={tick} pos={:?} path={:?} cursor={} contact={contact:?}",m.pos,m.path,m.cursor);
        }
        for wall in &walls { assert!(Obstacle::of(&m, &body).penetration(wall) <= HULL_CONTACT_SLOP); }
        if m.destination.is_none() { break; }
    }
    assert!(m.destination.is_none(),"commander factory loop: {:?} {:?} {}",m.pos,m.path,m.cursor);
}

#[test]
fn mixed_building_yard_has_non_overlapping_destinations() {
    let layout = modding::range::RangeLayout::from_ron(include_str!(
        "../../../data/range-motion-buildings.ron")).unwrap();
    let goals: Vec<_> = layout.course.as_ref().unwrap().patrols.iter().map(|p| {
        let def = &layout.units[p.unit];
        let at = [p.points[0][0], def.at[1], p.points[0][1]];
        let u = super::super::RangeUnit::of_kind(def.kind, at);
        Obstacle::of_unit(&u)
    }).collect();
    assert_eq!(goals.len(), 8);
    for i in 0..goals.len() {
        for j in i+1..goals.len() {
            assert_eq!(goals[i].penetration(&goals[j]), Fx::ZERO,
                "impossible overlapping destinations {i}/{j}");
        }
    }
}

#[test]
fn biped_keeps_clear_translation_along_a_moving_hull() {
    let w = World::new(1600, 512, 1600, 65536, Box::new(Flat));
    w.prepare_navigation([4,5]);
    let u = super::super::RangeUnit::of_kind(RangeUnitKind::Commander,[700,16,800]);
    let body = u.body;
    let mut m = u.motion;
    m.enabled = true;
    m.destination = Some([1100,1100]);
    m.path = vec![[1100,1100]];
    m.drive.head = angle::FULL/8;
    m.drive.vel = Fx::from_int(20);
    let own = Obstacle::of(&m,&body);
    let wall = Obstacle { moving:true, height:Fx::ZERO, head:0,
        pos:[m.pos[0].add(own.half[0]).add(Fx::from_int(20)).sub(HULL_CONTACT_SLOP)
            .add(Fx::from_ratio(1,10)),Fx::from_int(16),Fx::from_int(800)],
        half:[20,100].map(Fx::from_int) };
    m.allow_start(false);
    m.step(&w,&body,true,true,crate::mobility::Mobility::intact(true),&[wall],0,98,
        &nav::vehicle::Rubble::new(),&Default::default());
    assert!(!m.blocked,"a clear forward tangent was discarded with the blocked component");
    assert_eq!(m.pos[0],Fx::from_int(700));
    assert!(m.pos[2]>Fx::from_int(800));
    assert!(m.drive.vel>Fx::ZERO && m.drive.vel<=Fx::from_int(m.speed));
    assert!(Obstacle::of(&m,&body).penetration(&wall)<=HULL_CONTACT_SLOP);
    assert_eq!(m.path,vec![[1100,1100]],"dynamic avoidance replaced the route");
}

#[test]
fn a_tank_drives_across_a_supported_one_cell_ditch() {
    struct Ditch;
    impl voxel::Generator for Ditch {
        fn material(&self,x:u32,y:u32,_:u32)->u16 {
            u16::from(y<if x==150 {4} else {16})*2
        }
    }
    let w=World::new(512,128,256,65536,Box::new(Ditch));
    w.prepare_navigation([4,5]);
    let u=super::super::RangeUnit::of_kind(RangeUnitKind::Tank,[110,16,128]);
    let body=u.body;
    let mut m=u.motion;
    m.queue_move(&w,[280,128]);
    for tick in 0..180 {
        m.allow_start(true);
        m.step(&w,&body,true,true,crate::mobility::Mobility::intact(true),&[],tick,98,
            &nav::vehicle::Rubble::new(),&Default::default());
        assert!(m.pos[1].sub(Fx::from_int(16)).abs()<Fx::ONE,
            "supported track fell into a one-cell ditch at {tick}: {:?}",m.pos);
        if m.destination.is_none(){break;}
    }
    assert!(m.destination.is_none(),"supported ditch stranded tank at {:?}, {:?}",m.pos,m.path);
}
