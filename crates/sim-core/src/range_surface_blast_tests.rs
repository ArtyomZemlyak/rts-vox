use super::*;

#[test]
fn player_artillery_surface_hit_digs_soil_on_the_open_side_of_a_steel_wall() {
    steel_wall_impact(false);
}

#[test]
fn ai_artillery_surface_hit_digs_soil_on_the_open_side_of_a_steel_wall() {
    steel_wall_impact(true);
}

fn steel_wall_impact(ai: bool) {
    let mut layout = arena(RangeUnitKind::Artillery).layout.unwrap();
    layout.blocks.push(modding::range::RangeBlock {
        min: [420, 0, 64],
        max: [424, 192, 600],
        material: 3,
    });
    if ai {
        layout.units[0].allied = false;
        layout.units[1].allied = true;
        layout.units[1].at[0] = 470;
    } else {
        layout.battle = false;
        layout.gun_kind = RangeUnitKind::Artillery;
        layout.gun = [170, 16, 336];
        layout.target = [450, 24, 336];
        layout.airburst_on_target = false;
        layout.units.clear();
    }
    let config = ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    let mut r = BlastRange::with_layout(config, layout).unwrap();
    if ai {
        r.start_battle().unwrap();
        assert!(r.battle_order(1, Order::Hold, None));
    }
    let mut replay: Option<BlastRange> = None;
    let mut impact = None;
    let mut soil = 0;
    let mut behind = 0;
    let mut surface = std::collections::BTreeSet::new();
    for _ in 0..350 {
        if ai && r.battle.actors[0].fired > 0 {
            r.battle.actors[0].order = Order::Hold;
            step(&mut r);
        } else {
            r.step();
        }
        if let Some(copy) = replay.as_mut() {
            if ai && copy.battle.actors[0].fired > 0 {
                copy.battle.actors[0].order = Order::Hold;
                step(copy);
            } else {
                copy.step();
            }
            assert_eq!(r.state_hash(), copy.state_hash(), "surface wave replay");
        }
        if replay.is_none() && (r.explosion.is_some() || !r.battle.waves.is_empty()) {
            let bytes = r.snapshot();
            let copy = BlastRange::restore(r.config, &bytes).expect("surface wave snapshot");
            assert_eq!(bytes, copy.snapshot());
            replay = Some(copy);
        }
        if ai {
            for hit in &r.battle.impacts {
                impact.get_or_insert(hit.at);
            }
        } else if let Some(at) = r.impact {
            impact.get_or_insert(at);
        }
        for &(edit, before) in r.world.completed() {
            if edit.mat == 0 && matches!(before, 1 | 7 | 8) {
                soil += 1;
                behind += usize::from(edit.x >= 424);
                if edit.y == 15 {
                    surface.insert([edit.x, edit.y, edit.z]);
                }
            }
        }
        assert!(r.matter.economy.ledger.balanced());
    }
    eprintln!("steel surface ai={ai} impact={impact:?} soil={soil} behind={behind} surface={} ground={:?}", surface.len(), r.matter.admission.ground);
    assert!(
        replay.is_some(),
        "must replay the saved initial wave frontier"
    );
    assert_eq!(impact.unwrap()[0], 420, "the shell must meet the wall");
    if ai {
        assert_eq!(r.battle.actors[0].fired, 1);
        assert_eq!(r.battle.actors[1].fired, 0);
    }
    assert_eq!(
        r.world.material(420, 30, 336),
        3,
        "intact steel must remain"
    );
    assert_eq!(behind, 0, "the sealed wall must still shield its far side");
    assert!(
        surface
            .iter()
            .any(|p| r.world.material(p[0], p[1], p[2]) == 0),
        "a surface explosion vanished into steel instead of digging adjacent soil"
    );
}
