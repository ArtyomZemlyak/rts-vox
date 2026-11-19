use super::*;

#[test]
fn independent_oblique_aim_remains_solid_after_successive_actual_hit_removals() {
    for degrees in [0, 30, 75, 120, 165, 210, 255, 300, 345] {
        let mut r = arena_of([RangeUnitKind::TurretT1, RangeUnitKind::Tank]);
        let u = &mut r.units[1];
        let yaw = fx::angle::FULL * degrees / 360;
        u.motion.drive.head = yaw;
        u.update_pose(yaw);
        for shot in 0..8 {
            let aim = aim_point(u);
            assert!(u.solid(aim), "yaw {degrees} removal {shot}: aim {aim:?} is empty");
            let (segment, p) = u.hit(aim).unwrap();
            if segment == 0 {
                assert!(u.body.remove(p[0], p[1], p[2]));
                u.removed.push(p.map(|v| v as u8));
            } else {
                assert!(u.body.segments[segment - 1].body.remove(p[0], p[1], p[2]));
                u.seg_removed.push((segment as u8 - 1, p.map(|v| v as u8)));
            }
        }
    }
}
