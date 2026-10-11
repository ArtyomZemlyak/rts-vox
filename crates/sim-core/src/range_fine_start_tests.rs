//! Fine searches set up unwalked and walked under the range's start
//! allowance, and hulls led to a lattice entry (ADR-691, analysis 261).
use crate::blast::{battle::Order, BlastRange};
use modding::explosion::ExplosionConfig;
use modding::range::{RangeLayout, RangeUnitDef, RangeUnitKind};

/// Short goals: under `global_min_distance`, the fine planner's own.
const GOALS: [[i32; 2]; 4] = [[440, 200], [440, 330], [440, 460], [440, 590]];

/// Four tanks in a flat field, free control, a start allowance of one
/// start slice: a tick admits one search's start at a time.
fn field() -> BlastRange {
    let mut l = RangeLayout::from_ron(include_str!("../../../data/fixture-drive.ron")).unwrap();
    l.units = (0..4)
        .map(|k| RangeUnitDef {
            kind: RangeUnitKind::Tank,
            allied: true,
            at: [260, 16, 200 + 130 * k],
            weapon: None,
        })
        .collect();
    let config = ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    let mut r = BlastRange::with_layout(config, l).unwrap();
    starve(&mut r);
    r.start_free_control().unwrap();
    r
}
/// Tuning is data, not snapshot state: a restored range is given it again.
fn starve(r: &mut BlastRange) {
    r.navigation.search_start_budget = r.navigation.search_nodes_per_tick;
}
/// The orders' way of starting: the search set up, none of its work done.
fn order_unwalked(r: &mut BlastRange) {
    let rubble = r.current_rubble();
    for (k, goal) in GOALS.into_iter().enumerate() {
        let u = &mut r.units[k];
        u.motion.defer_fine(true);
        assert!(u.motion.move_to(&r.world, goal, &u.body, &[], &rubble));
        u.motion.defer_fine(false);
        assert_eq!(u.motion.fine_work(), Some([0, 0]), "hull {k} walked");
        assert!(u.motion.starting());
    }
}

/// **Starts share the start allowance, and every one of them answers.**
/// No tick spends more on searches in their start than the allowance, nor
/// more than one start slice on one of them; the starts that do not fit
/// wait, and the queue does not stop: all four tanks get routes and drive.
#[test]
fn unwalked_starts_share_the_start_allowance_and_all_answer() {
    let mut r = field();
    order_unwalked(&mut r);
    let budget = r.navigation.search_start_budget;
    let slice = r.navigation.search_nodes_per_tick;
    let mut answered = [None; 4];
    for tick in 1..=200u32 {
        let before: Vec<Option<u32>> = (0..4)
            .map(|k| {
                let m = &r.units[k].motion;
                m.starting().then(|| m.fine_work().unwrap()[1])
            })
            .collect();
        r.step();
        let mut spent = 0;
        for (k, start) in before.iter().enumerate() {
            let Some(start) = *start else { continue };
            let m = &r.units[k].motion;
            // Poses probed this tick: by the search in flight, or by the
            // answer it gave.
            let now = m.fine_work().map_or(m.last_plan[1], |[_, poses]| poses);
            let poses = (now - start) as usize;
            assert!(poses <= slice, "hull {k} took {poses} at tick {tick}");
            spent += poses;
        }
        assert!(spent <= budget, "tick {tick}: {spent} over {budget}");
        for (k, a) in answered.iter_mut().enumerate() {
            let m = &r.units[k].motion;
            if a.is_none() && !m.searching() && !m.path.is_empty() {
                *a = Some(tick);
            }
        }
        if answered.iter().all(Option::is_some) {
            break;
        }
    }
    assert!(
        answered.iter().flatten().any(|&t| t > 1),
        "the allowance never held a start back: {answered:?}"
    );
    assert!(answered.iter().all(Option::is_some), "{answered:?}");
    for _ in 0..60 {
        r.step();
    }
    for k in 0..4 {
        assert_ne!(r.units[k].motion.pos[0], fx::Fx::from_int(260), "hull {k}");
    }
}

/// **A range saved while searches are in their start restores to the same
/// fight.** The unwalked searches and those partly through their goal
/// ring are saved with the hulls; each tick after the restore has the
/// hash of the range that went on.
#[test]
fn restore_mid_start_equals_continuous() {
    for (ticks, starting) in [(0, 4), (1, 2)] {
        let mut a = field();
        order_unwalked(&mut a);
        for _ in 0..ticks {
            a.step();
        }
        let now = (0..4).filter(|&k| a.units[k].motion.starting()).count();
        assert!(now >= starting, "{now} starting after {ticks}");
        let bytes = a.snapshot();
        let mut b = BlastRange::restore(a.config, &bytes).unwrap();
        starve(&mut b);
        assert_eq!(b.snapshot(), bytes);
        for tick in 0..120 {
            a.step();
            b.step();
            assert_eq!(a.state_hash(), b.state_hash(), "tick {tick} after {ticks}");
        }
    }
}

/// **Spider 22 of the world battle leaves its pad** (review 252). It
/// stands in a cleared pad with a low wall within its own corridor
/// radius: no corridor from where it stands is clear, and the whole
/// order went to the fine planner, which a spider's slice could not walk
/// across the map. The whole-map search now names a lattice entry, the
/// fine planner leads the spider there, and the order goes on from it.
#[test]
fn a_pinned_spider_is_led_to_a_lattice_entry_and_moves_on() {
    let preset = modding::range_skirmish::WorldBattle::from_ron(include_str!(
        "../../../data/battle-world.ron"
    ))
    .unwrap();
    let mut layout = RangeLayout::from_ron(include_str!("../../../data/range-km.ron")).unwrap();
    layout.battle = true;
    layout.course.as_mut().unwrap().patrols.clear();
    let spider = preset.units[22].clone();
    assert_eq!(spider.kind, RangeUnitKind::Spider);
    assert_eq!(spider.at, [2050, 16, 1100]);
    layout.units = vec![spider];
    layout.blocks.extend(preset.blocks);
    let config = ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    let mut r = BlastRange::with_layout(config, layout).unwrap();
    r.start_free_control().unwrap();
    let goal = [4134, 3598];
    let m = &r.units[0].motion;
    let rubble = r.current_rubble();
    let refusal = nav::vehicle::coarse::plan_explained(
        &r.world,
        &rubble,
        m.probe_shape(&r.units[0].body),
        [m.pos[0], m.pos[2]],
        goal,
        m.coarse_config(),
    );
    let Err(nav::vehicle::coarse::Refusal::Entry { at }) = refusal else {
        panic!(
            "expected a lattice entry, got {:?}",
            refusal.map(|p| p.path)
        );
    };
    assert!(r.battle_order(0, Order::Move, Some(goal)));
    let start = r.units[0].motion.pos;
    let mut moved = None;
    for tick in 1..=300u32 {
        r.step();
        let m = &r.units[0].motion;
        assert_eq!(m.destination, Some(goal), "the order stands");
        if m.pos != start {
            moved = Some(tick);
            break;
        }
    }
    let moved = moved.expect("the spider never left its pad");
    // It drives the prefix to the entry and asks for the rest from there.
    let mut far = false;
    for _ in 0..600 {
        r.step();
        let m = &r.units[0].motion;
        let p = [m.pos[0].to_int_trunc(), m.pos[2].to_int_trunc()];
        if (p[0] - 2050).abs().max((p[1] - 1100).abs()) > 160 {
            far = true;
            break;
        }
    }
    assert!(far, "moved at {moved} toward {at:?}, but stayed by the pad");
}
