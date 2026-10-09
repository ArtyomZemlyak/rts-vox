use super::*;

#[test]
fn restored_experiments_match_fresh_worlds_and_future_ticks() {
    let config =
        ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
    let layout = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
    let validator = placement::Validator::default();
    let mut scene = BlastRange::with_layout(config, layout.clone()).unwrap();
    let mut cases = vec![layout.clone()];
    let mut next = layout.clone();
    next.units[0].allied = true;
    cases.push(next.clone());
    next.blocks[0].material = 5;
    cases.push(next.clone());
    next.blocks[0].material = layout.blocks[0].material;
    next.blocks.insert(
        1,
        modding::range::RangeBlock {
            min: [36, 16, 420],
            max: [80, 32, 460],
            material: 3,
        },
    );
    cases.push(next.clone());
    next.blocks.remove(1);
    next.zones.push(modding::range::RangeZone {
        name: "reset".into(),
        min: [64, 400],
        max: [192, 528],
        spawn: [112, 16, 448],
        target: [164, 24, 448],
        scenario: None,
    });
    next.course.as_mut().unwrap().ramps[0].max[1] += 2;
    cases.push(next.clone());
    next.clear_min[0] += 8;
    cases.push(next);
    for layout in cases {
        scene.control();
        for _ in 0..3 {
            scene.step();
        }
        scene.world.queue_dig(90, 8, 450);
        scene.world.flush();
        scene.world.queue_dig(91, 8, 450);
        let bits = scene.world.bits().as_ptr();
        let mut fresh = BlastRange::with_layout(config, layout.clone()).unwrap();
        scene = scene
            .rebuilt_with_layout(config, layout, &validator)
            .unwrap();
        assert_eq!(bits, scene.world.bits().as_ptr());
        assert_eq!(scene.world.bits(), fresh.world.bits());
        assert_eq!(scene.state_hash(), fresh.state_hash());
        for x in (0..scene.size()[0]).step_by(13) {
            for z in (0..scene.size()[2]).step_by(17) {
                assert_eq!(
                    scene.world.column_ceiling(x, z),
                    fresh.world.column_ceiling(x, z)
                );
                let mut a = [0; 192];
                let mut b = [0; 192];
                scene.world.column(x, z, 0, &mut a);
                fresh.world.column(x, z, 0, &mut b);
                assert_eq!(a, b);
            }
        }
        scene.control();
        fresh.control();
        for _ in 0..12 {
            scene.step();
            fresh.step();
            assert_eq!(scene.state_hash(), fresh.state_hash());
        }
    }
}

#[test]
fn resized_range_rebuild_matches_a_fresh_world() {
    let config =
        ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
    let layout = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
    let validator = placement::Validator::default();
    let mut scene = BlastRange::with_layout(config, layout.clone()).unwrap();
    let mut larger = layout;
    larger.size[0] += 128;
    larger.size[2] += 128;
    let fresh = BlastRange::with_layout(config, larger.clone()).unwrap();
    scene = scene
        .rebuilt_with_layout(config, larger, &validator)
        .unwrap();
    assert_eq!(scene.state_hash(), fresh.state_hash());
    assert_eq!(scene.world.bits(), fresh.world.bits());
}
