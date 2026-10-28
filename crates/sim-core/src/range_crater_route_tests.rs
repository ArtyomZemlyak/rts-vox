use super::*;
use modding::range::RangeUnitKind;

struct Flat;
impl voxel::Generator for Flat {
    fn material(&self, _x: u32, y: u32, _z: u32) -> u16 {
        u16::from(y < 64)
    }
}
fn accepted(body: &UnitBody, kind: RangeUnitKind) -> Motion {
    let mut m = Motion::new([120, 64, 320], kind, body);
    m.enabled = true;
    m.destination = Some([520, 320]);
    m.path = vec![[520, 320]];
    m.drive.vel = Fx::from_int(20);
    m.navigation.global_nodes = 0;
    m
}
fn advance(m: &mut Motion, w: &World, b: &UnitBody, tick: u32, changed: &nav::vehicle::Changed) {
    m.step(
        w,
        b,
        true,
        true,
        crate::mobility::Mobility::intact(true),
        &[],
        tick,
        98,
        &nav::vehicle::Rubble::new(),
        changed,
    );
}

#[test]
fn crater_route_reacts_before_contact_preserves_order_and_brakes_while_queued() {
    for (kind, body) in [
        (RangeUnitKind::Tank, voxel::unit::tank()),
        (RangeUnitKind::Artillery, voxel::unit::artillery()),
        (RangeUnitKind::Strider, voxel::unit::strider()),
        (RangeUnitKind::Spider, voxel::unit::spider()),
    ] {
        let w = World::new(640, 128, 640, 100, Box::new(Flat));
        let mut m = accepted(&body, kind);
        m.next_plan = 500;
        let mut dirty = nav::vehicle::Changed::default();
        dirty.column(280, 320);
        m.allow_start(false);
        let velocity = m.drive.vel;
        advance(&mut m, &w, &body, 1, &dirty);
        assert!(m.path.is_empty(), "{kind:?} kept a stale route");
        assert_eq!(m.destination, Some([520, 320]));
        assert_eq!(m.next_plan, 1, "waited for the old retry timer");
        assert!(!m.started_search(), "bypassed the shared admission cap");
        assert!(
            m.drive.vel <= velocity,
            "accelerated towards the changed floor"
        );
        let mut bytes = Blob::new();
        m.save(&mut bytes);
        let mut copy = Motion::new([120, 64, 320], kind, &body);
        assert!(copy.load(&mut Scan::new(&bytes.v)));
        let mut after = Blob::new();
        copy.save(&mut after);
        assert_eq!(after.v, bytes.v);
        assert!(copy.wants_start(2));
    }
}

#[test]
fn crater_route_ignores_remote_edits_even_inside_a_diagonal_bounding_box() {
    let body = voxel::unit::tank();
    let mut m = accepted(&body, RangeUnitKind::Tank);
    m.path = vec![[520, 520]];
    m.destination = Some([520, 520]);
    m.pos[2] = Fx::from_int(120);
    let mut before = Blob::new();
    m.save(&mut before);
    let mut dirty = nav::vehicle::Changed::default();
    dirty.column(160, 450);
    m.terrain_changed(&body, 2, &dirty);
    let mut after = Blob::new();
    m.save(&mut after);
    assert_eq!(before.v, after.v);
    assert_eq!(m.last_plan, [0, 0]);
}

#[test]
fn crater_route_detours_a_new_deep_pit_and_finishes_for_tracks_and_legs() {
    for (kind, body) in [
        (RangeUnitKind::Tank, voxel::unit::tank()),
        (RangeUnitKind::Artillery, voxel::unit::artillery()),
        (RangeUnitKind::Strider, voxel::unit::strider()),
    ] {
        let mut w = World::new(640, 128, 640, 65536, Box::new(Flat));
        let mut m = accepted(&body, kind);
        for tick in 0..20 {
            advance(&mut m, &w, &body, tick, &Default::default());
        }
        let mut dirty = nav::vehicle::Changed::default();
        for z in 260..380 {
            for x in 260..340 {
                dirty.column(x, z);
                for y in 16..64 {
                    w.queue_edit(voxel::Edit::dig_if(x as u32, y, z as u32, 1));
                }
            }
        }
        while w.queued() > 0 {
            w.flush();
        }
        m.terrain_changed(&body, 20, &dirty);
        assert!(m.path.is_empty());
        let mut detour = Fx::ZERO;
        let mut low = m.pos[1];
        for tick in 20..1800 {
            m.allow_start(true);
            advance(&mut m, &w, &body, tick, &Default::default());
            detour = detour.max(m.pos[2].sub(Fx::from_int(320)).abs());
            low = low.min(m.pos[1]);
            if m.destination.is_none() {
                break;
            }
        }
        assert!(
            m.destination.is_none(),
            "{kind:?} stalled at {:?}, {:?}, {:?}",
            m.pos,
            m.last_plan,
            m.path
        );
        assert!(
            detour >= Fx::from_int(60),
            "{kind:?} failed to skirt the pit"
        );
        assert!(low > Fx::from_int(60), "{kind:?} drove into the deep pit");
    }
}

#[test]
#[ignore = "manual movement comparison; run alone in release"]
fn bench_crater_route_remote_changes() {
    use std::time::Instant;
    let body = voxel::unit::tank();
    let w = World::new(640, 128, 640, 100, Box::new(Flat));
    let source: Vec<_> = (0..64)
        .map(|_| accepted(&body, RangeUnitKind::Tank))
        .collect();
    let mut dirty = nav::vehicle::Changed::default();
    dirty.column(600, 600);
    let mut states = Vec::new();
    for changed in [nav::vehicle::Changed::default(), dirty] {
        let mut units = source.clone();
        let start = Instant::now();
        for tick in 0..180 {
            for m in &mut units {
                advance(m, &w, &body, tick, &changed);
            }
        }
        println!(
            "CRATER_ROUTE remote={} units=64 ticks=180 elapsed_us={}",
            !changed.is_empty(),
            start.elapsed().as_micros()
        );
        let mut b = Blob::new();
        for m in &units {
            m.save(&mut b);
        }
        states.push(b.v);
    }
    assert_eq!(states[0], states[1]);
}
