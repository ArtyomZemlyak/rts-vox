use super::*;

#[test]
fn parameter_export_is_owned_fresh_and_does_not_mutate_physics() {
    fn send_sync<T: Send + Sync>() {}
    send_sync::<Parameters>();
    let mut sim = world();
    let before = sim.snapshot();
    let mut view = Parameters::new(&sim, 5, [2, 1, 2, u16::MAX].into_iter());
    assert_eq!(before, sim.snapshot());
    assert_eq!(
        view.materials.iter().map(|m| m.id).collect::<Vec<_>>(),
        vec![1, 2]
    );
    assert!(view.material(u16::MAX).is_none());
    let old = view.clone();
    let capacity = view.materials.capacity();
    sim.config.energy += 1;
    sim.gun.motion.speed += 1;
    sim.units[0].motion.chassis.thrust += 1;
    sim.matter.tuning.drag_pct += 1;
    let mut material = *sim.matter.economy.mats.get(1).unwrap();
    material.density += 1;
    sim.matter.economy.mats.replace(material).unwrap();
    view.update(&sim, 5, [2, 1, 2, u16::MAX].into_iter());
    assert_eq!(view.tick, old.tick);
    assert_eq!(view.weapon.energy, old.weapon.energy + 1);
    assert_eq!(view.gun.speed, old.gun.speed + 1);
    assert_eq!(
        view.motions[RangeUnitKind::Tank.index()]
            .unwrap()
            .chassis
            .thrust,
        old.motions[RangeUnitKind::Tank.index()]
            .unwrap()
            .chassis
            .thrust
            + 1
    );
    assert_eq!(view.debris.drag_pct, old.debris.drag_pct + 1);
    assert_eq!(
        view.material(1).unwrap().density,
        old.material(1).unwrap().density + 1
    );
    assert_eq!(view.materials.capacity(), capacity);
    drop(sim);
    assert_ne!(old.weapon.energy, view.weapon.energy);
}

#[test]
fn parameter_export_preserves_first_slot_and_clears_missing_data_on_reset() {
    let mut sim = world();
    sim.units[0].status.alive = false;
    sim.units[0].motion.speed = 123;
    sim.units[1].motion.speed = 456;
    let course =
        modding::range::RangeLayout::from_ron(include_str!("../../../data/range-cover.ron"))
            .unwrap()
            .course;
    assert!(course.is_some());
    sim.layout.as_mut().unwrap().course = course;
    let mut view = Parameters::new(&sim, 5, [1, 2].into_iter());
    assert_eq!(
        view.motions[RangeUnitKind::Tank.index()].unwrap().speed,
        123
    );
    assert!(view.motions[RangeUnitKind::Strider.index()].is_none());
    let course = sim.layout.as_ref().unwrap().course.as_ref().unwrap();
    assert_eq!(view.course, Some([course.walker_speed, course.step_height]));
    sim.layout = None;
    sim.units.clear();
    view.update(&sim, 6, [u16::MAX].into_iter());
    assert_eq!((view.generation, view.tick), (6, 0));
    assert!(view.motions.iter().all(Option::is_none));
    assert!(view.materials.is_empty());
    assert!(view.course.is_none());
}
