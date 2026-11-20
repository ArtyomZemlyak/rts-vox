//! Contact-to-crown checks through the shared battle pipeline, without gunfire.
use super::ai::{AiOrder, Decision};
use super::*;
use modding::range::{RangeBlock, RangeUnitDef};

fn grove(kind: RangeUnitKind, allied: bool) -> BlastRange {
    grove_with_height(kind, allied, false)
}

fn grove_with_height(kind: RangeUnitKind, allied: bool, tall: bool) -> BlastRange {
    let mut l = RangeLayout::from_ron(include_str!("../../../data/fixture-drive.ron")).unwrap();
    l.clear_min = [40, 40];
    l.clear_max = [728, 728];
    l.course = None;
    l.shields.clear();
    l.blocks.clear();
    // Separate rooted trees, with a crown above the contact region. A low
    // yielding slab would prove only contact capture, not crown propagation.
    let crown = if tall { 49 } else { 23 };
    for z in (80..=304).step_by(16) {
        l.blocks.extend([
            RangeBlock {
                min: [220, 16, z],
                max: [222, crown, z + 2],
                material: 5,
            },
            RangeBlock {
                min: [218, crown, z - 2],
                max: [224, crown + 4, z + 4],
                material: 4,
            },
        ]);
    }
    l.units = (0..6)
        .map(|i| RangeUnitDef {
            kind: if i == 0 { kind } else { RangeUnitKind::Tank },
            at: if i == 0 {
                [128, 16, 192]
            } else {
                [300 + i * 60, 16, 540]
            },
            allied: if i == 0 { allied } else { !allied },
            weapon: None,
        })
        .collect();
    if kind.index() >= 14 {
        l.size = [4096, 2048, 4096];
        l.clear_min = [64, 64];
        l.clear_max = [4032, 4032];
        l.units[0].at = [1000, 16, 1600];
        for (i, def) in l.units.iter_mut().enumerate().skip(1) {
            def.at = [2800 + i as i32 * 100, 16, 3000];
        }
        for block in &mut l.blocks {
            block.min[0] += 1780;
            block.max[0] += 1780;
            block.min[2] += 1408;
            block.max[2] += 1408;
        }
    }
    let config = ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    let mut r = BlastRange::with_layout(config, l).unwrap();
    assert!(r.set_navigation_workers(4)); // Six slots exercise deferred feet.
    assert!(r.set_physics_workers(4));
    r.start_battle().unwrap();
    for actor in &mut r.battle.actors {
        actor.order = Order::Hold;
        // A Move order may fire at hostiles along its route. Disable reload
        // admission so this fixture measures only physical plant contacts.
        actor.next = u32::MAX;
    }
    r
}

#[test]
fn tall_tree_crowns_above_the_felling_radius_collapse_after_walker_contact() {
    for kind in [RangeUnitKind::Strider, RangeUnitKind::Spider] {
        let mut r = grove_with_height(kind, true, true);
        assert!(r.battle_order(0, Order::Move, Some([350, 192])));
        let mut stems = std::collections::BTreeSet::new();
        for _ in 0..360 {
            tick_without_fire(&mut r, true);
            for p in &r.units[0].motion.cut {
                if p[0] >= 220 && p[0] < 222 && p[1] < 23 {
                    stems.insert(80 + (p[2] - 80) / 16 * 16);
                }
            }
            if !stems.is_empty() {
                break;
            }
        }
        assert!(!stems.is_empty(), "{kind:?} did not cut a tall trunk");
        r.battle.actors[0].order = Order::Hold;
        r.units[0].motion.stop();
        r.units[0].motion.enabled = false;
        r.units[0].motion.pos[0] = Fx::from_int(350);
        r.units[0].update_pose(0);
        let mut replay = BlastRange::restore(r.config, &r.snapshot()).unwrap();
        assert!(replay.set_navigation_workers(1));
        for _ in 0..90 {
            tick_without_fire(&mut r, false);
            tick_without_fire(&mut replay, false);
            assert_eq!(r.state_hash(), replay.state_hash());
        }
        assert!(
            r.collapsed_cells > 0,
            "{kind:?}: upper crown never entered structural collapse"
        );
        for z in stems {
            assert!(
                (49..53).all(|y| (z - 2..z + 4)
                    .all(|qz| (218..224).all(|x| r.world.material(x, y, qz) == 0))),
                "{kind:?}: crown above radius remained hanging at z={z}"
            );
        }
    }
}

fn tick_without_fire(r: &mut BlastRange, moving: bool) {
    // Feed a real scheduled enemy plan rather than changing its Motion from
    // the test. Allied orders go through the ordinary player executor.
    if r.battle.ai.next_tick == r.tick {
        let plan = r
            .units
            .iter()
            .enumerate()
            .filter(|(i, _)| !r.allied(*i))
            .map(|(i, _)| Decision {
                actor: i,
                order: if i == 0 && moving {
                    AiOrder::Move([350, 192])
                } else {
                    AiOrder::Hold
                },
            })
            .collect();
        r.submit_enemy_ai_plan(r.tick, plan).unwrap();
    }
    r.step();
    assert!(
        r.battle.actors.iter().all(|a| a.fired == 0),
        "a weapon felled trees on the mover's behalf"
    );
    assert!(r.matter.economy.ledger.balanced());
}

fn contact_and_crown(kind: RangeUnitKind) {
    for allied in [true, false] {
        for refusal in ["none", "flying", "claims"] {
            let full_pool = refusal != "none";
            let mut a = grove(kind, allied);
            a.navigation.fell.dust_materials = [0]; // Keep the original full-pool/32-tick assertions.
            if refusal == "flying" {
                a.matter.tuning.max_flying = 0;
            }
            if refusal == "claims" {
                // No claim slots: unlike a full flying pool this used to
                // disable contact capture before Matter could retain it.
                a.matter.tuning.max_claims = 0;
            }
            if allied {
                assert!(a.battle_order(0, Order::Move, Some([350, 192])));
            }
            let mut touched = std::collections::BTreeSet::new();
            for _ in 0..360 {
                tick_without_fire(&mut a, true);
                touched.extend(a.units[0].motion.cut.iter().copied());
                // A crown extends beyond the trunk: first contact need not
                // reach wood. Continue until a real trunk was touched.
                if touched.iter().any(|p| p[1] < 23) {
                    break;
                }
            }
            assert!(
                !touched.is_empty(),
                "{kind:?} allied={allied} refusal={refusal} never contacted a tree, at {:?}",
                a.units[0].at
            );
            assert!(
                touched.iter().any(|p| p[1] < 23),
                "{kind:?} only touched a crown"
            );
            if full_pool {
                assert!(
                    a.matter.felling_pending() >= touched.len(),
                    "{kind:?} lost rejected contacts"
                );
                assert!(touched
                    .iter()
                    .all(|p| matches!(a.world.material(p[0], p[1], p[2]), 4 | 5)));
            }
            // Departure rules out a later footprint quietly repairing a lost
            // contact. Stop the AI order too, not just its current velocity.
            a.battle.actors[0].order = Order::Hold;
            a.units[0].motion.stop();
            a.units[0].motion.enabled = false;
            a.units[0].motion.pos[0] = Fx::from_int(350);
            a.units[0].update_pose(0);
            let bytes = a.snapshot();
            let mut b = BlastRange::restore(a.config, &bytes).expect("felling snapshot");
            assert_eq!(b.snapshot(), bytes);
            assert!(b.set_navigation_workers(1));
            assert!(b.set_physics_workers(1));
            for r in [&mut a, &mut b] {
                r.navigation.fell.dust_materials = [0];
                r.matter.tuning.max_flying = 4096;
                r.matter.tuning.max_claims = 4096;
            }
            for _ in 0..32 {
                tick_without_fire(&mut a, false);
                tick_without_fire(&mut b, false);
                assert_eq!(
                    a.state_hash(),
                    b.state_hash(),
                    "{kind:?} allied={allied} full_pool={full_pool} workers/replay"
                );
            }
            assert!(
                touched
                    .iter()
                    .all(|p| a.world.material(p[0], p[1], p[2]) == 0),
                "{kind:?} left contacted vegetation standing"
            );
            assert!(a.removed_materials[5] > 0, "{kind:?} never cut a trunk");
            assert!(
                a.removed_materials[4] > 0,
                "{kind:?} left the crown hanging"
            );
            assert!(a.matter.spent_mj > 0 && a.matter.spent_mj <= a.matter.supplied_mj);
            eprintln!("{kind:?} allied={allied} full_pool={full_pool}: contacts={} trunk={} foliage={} pending={}", touched.len(), a.removed_materials[5], a.removed_materials[4], a.matter.felling_pending());
        }
    }
}

#[test]
fn tank_contact_fells_trunk_and_crown_on_both_sides() {
    contact_and_crown(RangeUnitKind::Tank);
}
#[test]
fn strider_contact_fells_trunk_and_crown_on_both_sides() {
    contact_and_crown(RangeUnitKind::Strider);
}
#[test]
fn spider_contact_fells_trunk_and_crown_on_both_sides() {
    contact_and_crown(RangeUnitKind::Spider);
}
#[test]
fn artillery_contact_fells_trunk_and_crown_on_both_sides() {
    contact_and_crown(RangeUnitKind::Artillery);
}
#[test]
fn artillery_t2_contact_fells_trunk_and_crown_on_both_sides() {
    contact_and_crown(RangeUnitKind::ArtilleryT2);
}

#[test]
fn emplacements_reject_movement_and_do_not_remotely_fell_trees() {
    let mut checked = 0;
    for kind in RangeUnitKind::ALL.into_iter().filter(|k| k.emplacement()) {
        let mut r = grove(kind, true);
        let at = r.units[0].at;
        assert!(
            !r.battle_order(0, Order::Move, Some([350, 192])),
            "{kind:?} accepted movement"
        );
        for _ in 0..8 {
            tick_without_fire(&mut r, false);
        }
        assert_eq!(r.units[0].at[0], at[0]);
        assert_eq!(r.units[0].at[2], at[2]);
        assert!(r.units[0].motion.cut.is_empty());
        assert_eq!(r.removed_materials[4] + r.removed_materials[5], 0);
        checked += 1;
    }
    assert_eq!(checked, 15); // Legacy emplacements, Razlom, Greben, Kil T1–T4 and power plants T1–T3.
}

#[test]
fn every_armed_kind_damages_wood_with_its_stock_weapon() {
    for kind in RangeUnitKind::ALL {
        let mut l = grove(RangeUnitKind::Tank, true).layout.take().unwrap();
        l.units[0].kind = kind;
        l.blocks = vec![
            RangeBlock {
                min: [400, 16, 180],
                max: [404, 70, 204],
                material: 5,
            },
            RangeBlock {
                min: [398, 70, 178],
                max: [406, 74, 206],
                material: 4,
            },
        ];
        let mut aim = [402, 45, 192];
        let (lo, hi) = RangeUnit::of_kind(kind, l.units[0].at).bounds();
        if (0..3).any(|a| lo[a] < 1 || hi[a] >= l.size[a] as i32 - 1) {
            l.size = [4096, 2048, 4096];
            l.clear_max = [4032, 4032];
            l.units[0].at = [1100, 16, 1600];
            for (i, def) in l.units.iter_mut().enumerate().skip(1) {
                def.at = [2800 + i as i32 * 100, 16, 2800];
            }
            l.blocks = vec![RangeBlock {
                min: [2100, 16, 1588],
                max: [2104, 300, 1612],
                material: 5,
            }];
            aim = [2102, 140, 1600];
        }
        let config =
            ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
        let mut r = BlastRange::with_layout(config, l).unwrap();
        r.start_battle().unwrap();
        for a in &mut r.battle.actors {
            a.order = Order::Hold;
            a.next = u32::MAX;
        }
        r.battle.actors[0].next = 0;
        let at = r.units[0].at;
        let armed = r.battle_weapon(0).is_some();
        assert!(r.battle_order(0, Order::Ground(aim), None));
        let mut replay: Option<BlastRange> = None;
        for _ in 0..600 {
            if r.battle.ai.next_tick == r.tick {
                let plan = (1..r.units.len())
                    .map(|actor| Decision {
                        actor,
                        order: AiOrder::Hold,
                    })
                    .collect();
                r.submit_enemy_ai_plan(r.tick, plan).unwrap();
            }
            r.step();
            assert!(
                r.units.iter().all(|u| u.motion.cut.is_empty()),
                "{kind:?}: physical contact contaminated gunfire fixture"
            );
            assert_eq!(
                [r.units[0].at[0], r.units[0].at[2]],
                [at[0], at[2]],
                "{kind:?} moved instead of firing"
            );
            if let Some(copy) = replay.as_mut() {
                if copy.battle.ai.next_tick == copy.tick {
                    let plan = (1..copy.units.len())
                        .map(|actor| Decision {
                            actor,
                            order: AiOrder::Hold,
                        })
                        .collect();
                    copy.submit_enemy_ai_plan(copy.tick, plan).unwrap();
                }
                BlastRange::step(copy);
                assert_eq!(r.state_hash(), copy.state_hash());
            } else if r.battle.actors[0].fired > 0 {
                replay = Some(BlastRange::restore(config, &r.snapshot()).unwrap());
            }
            assert!(r.matter.economy.ledger.balanced());
            if r.removed_materials[5] > 0 || !armed {
                break;
            }
        }
        if !armed {
            assert_eq!(r.battle_weapon(0), None);
            assert_eq!(r.battle.actors[0].fired, 0);
            assert_eq!(r.removed_materials[5], 0);
        } else {
            assert!(r.battle.actors[0].fired > 0, "{kind:?} never fired");
            assert!(
                r.removed_materials[5] > 0,
                "{kind:?} weapon never removed wood"
            );
            assert!(replay.is_some());
        }
        eprintln!(
            "{kind:?} stock={:?} fired={} wood={} burned={}",
            r.battle_weapon(0),
            r.battle.actors[0].fired,
            r.removed_materials[5],
            r.heat.burned_by_material[5]
        );
    }
}


#[test]
fn mixed_felling_reopens_after_restore_on_one_real_tree() {
    let mut reference=None;
    for workers in [1,2,4,8] {
    let mut hashes=Vec::new();
    let mut layout = grove(RangeUnitKind::Tank, true).layout.take().unwrap();
    layout.blocks.retain(|b| (190..=192).contains(&b.min[2]));
    let config = ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    let mut a = BlastRange::with_layout(config, layout).unwrap();
    a.start_battle().unwrap();
    assert!(a.set_navigation_workers(workers));assert!(a.set_physics_workers(workers));
    a.matter.tuning.max_flying = 0;
    assert!(a.battle_order(0, Order::Move, Some([350, 192])));
    let mut contacted = Vec::new();
    for _ in 0..180 {
        tick_without_fire(&mut a, true);
        hashes.push(a.state_hash());
        contacted.extend(a.units[0].motion.cut.iter().copied());
        if contacted.iter().any(|p| p[1] < 23) {
            break;
        }
    }
    assert!(contacted.iter().any(|p| p[1] < 23));
    assert!(a.matter.felling_pending() > 0);
    assert_eq!(a.matter.spent_mj,0,"a full physical pool cannot pay an unadmitted leaf cut");
    a.battle.actors[0].order = Order::Hold;
    a.units[0].motion.stop();
    a.units[0].motion.enabled = false;
    a.units[0].motion.pos[0] = Fx::from_int(350);
    a.units[0].update_pose(0);
    let bytes = a.snapshot();
    let mut b = BlastRange::restore(config, &bytes).unwrap();
    assert_eq!(b.snapshot(), bytes);
    for r in [&mut a, &mut b] {
        r.matter.tuning.max_flying = 4096;
        r.matter.tuning.max_claims = 4096;

    }
    let other=if workers==1 {8}else{1};
    assert!(b.set_navigation_workers(other));assert!(b.set_physics_workers(other));
    for _ in 0..300 {
        tick_without_fire(&mut a, false);
        tick_without_fire(&mut b, false);
        hashes.push(a.state_hash());
        assert_eq!(a.state_hash(), b.state_hash());
        assert!(a.matter.economy.ledger.balanced());
        if a.matter.felling_pending() == 0 && a.world.queued() == 0 {
            break;
        }
    }
    assert_eq!(a.matter.felling_pending(), 0);
    assert!(a.removed_materials[4] > 0 && a.removed_materials[5] > 0);
    assert_eq!(a.matter.economy.sunk(crate::matter::Sink::Pulverised),0,"stock foliage must remain physical");
    assert!(a.matter.spent_mj>0,"reopened physical cuts must be paid");
    assert!(a.matter.debris.fragments.iter().any(|f| f.material == 4));
    assert!(a.matter.debris.fragments.iter().any(|f| f.material == 5));
    for p in contacted {
        assert_eq!(a.world.material(p[0], p[1], p[2]), 0, "contact remained at {p:?}");
    }
    if let Some(expected)=reference.as_ref(){assert_eq!(&hashes,expected,"physical foliage workers={workers}");}else{reference=Some(hashes);}
    }
}


#[test]
fn walker04_forest_traversal_keeps_bounded_fragments_and_worker_replay_hashes() {
    let config = ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    let mut reference = None;
    for workers in [1, 2, 4, 8] {
        let mut l = RangeLayout::from_ron(include_str!("../../../data/range-motion-yard.ron")).unwrap();
        l.size = [2048, 512, 2048];
        l.clear_max = [2048, 2048];
        l.blocks.clear();
        if let Some(c) = l.course.as_mut() {
            c.max = [2008, 2008];
            c.ramps.clear();
            c.patrols.truncate(1);
            c.patrols[0].unit = 0;
            c.patrols[0].points = vec![[1400, 1024], [700, 1024]];
        }
        let mut roots = Vec::new();
        for x in (900..1060).step_by(32) {
            for z in (976..1072).step_by(24) {
                l.blocks.push(RangeBlock {
                    min: [x, 16, z],
                    max: [x + 2, 36, z + 2],
                    material: 5,
                });
                l.blocks.push(RangeBlock {
                    min: [x - 3, 28, z - 3],
                    max: [x + 5, 40, z + 5],
                    material: 4,
                });
                roots.push([x as u32, 16, z as u32]);
            }
        }
        l.units = vec![RangeUnitDef {
            kind: RangeUnitKind::Walker04,
            at: [720, 16, 1024],
            allied: true,
            weapon: None,
        }];
        let mut sim = BlastRange::with_layout(config, l).unwrap();
        assert!(sim.set_navigation_workers(workers));
        assert!(sim.set_physics_workers(workers));
        sim.start_free_control().unwrap();
        assert!(sim.battle_order(
            0,
            Order::Pilot {
                throttle: 1,
                turn: 0,
                aim: [1600, 32, 1024],
                fire: false,
            },
            None,
        ));
        let mut replay: Option<BlastRange> = None;
        let mut hashes = Vec::new();
        let mut max_fragments = 0usize;
        for tick in 0..400 {
            sim.step();
            if let Some(copy) = replay.as_mut() {
                copy.step();
                assert_eq!(sim.state_hash(), copy.state_hash(), "workers={workers} tick={tick}");
            } else if tick == 65 {
                let mut copy = BlastRange::restore(config, &sim.snapshot()).unwrap();
                let alt = if workers == 1 { 4 } else { 1 };
                assert!(copy.set_navigation_workers(alt));
                assert!(copy.set_physics_workers(alt));
                replay = Some(copy);
            }
            assert!(sim.matter.economy.ledger.balanced());
            max_fragments = max_fragments.max(sim.matter.debris.fragments.len());
            hashes.push(sim.state_hash());
            if sim.units[0].motion.pos[0] > Fx::from_int(1068) { break; }
        }
        let felled = roots
            .iter()
            .filter(|&&[x, y, z]| sim.world.material(x, y, z) != 5)
            .count();
        assert!(felled >= 4, "Walker04 felled only {felled} trunks");
        assert!(max_fragments <= sim.matter.tuning.max_fragments as usize,
            "physical foliage exceeded the configured pool: {max_fragments}");
        assert!(sim.units[0].motion.pos[0] > Fx::from_int(1068),
            "Walker04 did not leave the authored grove: {:?}", sim.units[0].motion.pos);
        assert_eq!(sim.matter.economy.sunk(crate::matter::Sink::Pulverised), 0);
        assert!(sim.matter.debris.fragments.iter().any(|piece| piece.material == 4),
            "stock crowns must produce physical foliage, not just trunk fragments");
        if let Some(expected) = reference.as_ref() {
            assert_eq!(&hashes, expected, "worker desync at workers={workers}");
        } else {
            reference = Some(hashes);
        }
    }
}

#[test]
fn walker04_hilly_forest_move_matches_across_cold_worker_restores() {
    let config = ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    let base = RangeLayout::from_ron(include_str!("../../../data/range-motion-yard.ron")).unwrap();
    for kind in [RangeUnitKind::Walker04, RangeUnitKind::Spider] {
        let mut reference: Option<Vec<u64>> = None;
        for workers in [1, 2, 4, 8] {
            let mut l = base.clone();
            l.size = [1536, 512, 1536];
            l.zones.clear();
            l.roads.clear();
            l.mounds.clear();
            l.shields.clear();
            l.blocks.clear();
            l.course = None;
            l.clear_min = [280, 940];
            l.clear_max = [440, 1110];
            l.units = vec![RangeUnitDef {
                kind,
                at: [360, 16, 1024],
                allied: true,
                weapon: None,
            }];
            l.battle = true;
            let mut sim = BlastRange::with_layout(config, l).unwrap();
            assert!(sim.set_navigation_workers(workers));
            assert!(sim.set_physics_workers(workers));
            sim.start_free_control().unwrap();
            assert!(sim.battle_order(0, Order::Move, Some([1100, 1024])));
            let mut replay: Option<BlastRange> = None;
            let mut hashes = Vec::new();
            let mut min_up_y = Fx::ONE;
            for tick in 0..140 {
                sim.step();
                min_up_y = min_up_y.min(sim.units[0].motion.hull.up.y);
                if let Some(copy) = replay.as_mut() {
                    copy.step();
                    assert_eq!(
                        sim.state_hash(),
                        copy.state_hash(),
                        "{kind:?} workers={workers} tick={tick}"
                    );
                } else if tick == 25 {
                    let mut copy = BlastRange::restore(config, &sim.snapshot()).unwrap();
                    let alt = if workers == 1 { 4 } else { 1 };
                    assert!(copy.set_navigation_workers(alt));
                    assert!(copy.set_physics_workers(alt));
                    replay = Some(copy);
                }
                assert!(sim.matter.economy.ledger.balanced());
                hashes.push(sim.state_hash());
            }
            assert!(
                sim.units[0].motion.pos[0] > Fx::from_int(520),
                "{kind:?} must advance across hilly forest under Order::Move: {:?}",
                sim.units[0].motion.pos
            );
            if kind == RangeUnitKind::Walker04 {
                assert!(
                    min_up_y >= Fx::from_ratio(99, 100),
                    "Walker04 torso tilted past the hill-leveling cap: min_up_y={:?}",
                    min_up_y
                );
            }
            assert_eq!(sim.matter.economy.sunk(crate::matter::Sink::Pulverised), 0);
            assert!(
                sim.matter.debris.fragments.iter().any(|piece| piece.material == 4),
                "{kind:?} hilly forest felling must keep physical foliage fragments"
            );
            if let Some(expected) = reference.as_ref() {
                assert_eq!(&hashes, expected, "{kind:?} worker desync at workers={workers}");
            } else {
                reference = Some(hashes);
            }
        }
    }
}
