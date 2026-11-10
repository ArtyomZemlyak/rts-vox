//! Bounded steering preview. No persistent geometry or route cache.
use super::*;

#[derive(Clone, Copy)]
pub(super) struct Steering {
    pub delta: [Fx; 2],
    pub way: i32,
    pub alignment: Fx,
    pub passed: bool,
    pub rounded: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use modding::range::RangeUnitKind;

    struct Flat;
    impl voxel::Generator for Flat {
        fn material(&self, _: u32, y: u32, _: u32) -> u16 {
            if y < 16 {
                2
            } else {
                0
            }
        }
    }
    fn world() -> World {
        let w = World::new(640, 64, 640, 100000, Box::new(Flat));
        w.prepare_navigation([4, 5]);
        w
    }
    fn motion(kind: RangeUnitKind, body: &UnitBody, path: Vec<[i32; 2]>) -> Motion {
        let mut m = Motion::new([100, 16, 180], kind, body);
        m.enabled = true;
        m.destination = path.last().copied();
        m.path = path;
        m
    }
    fn step(m: &mut Motion, w: &World, b: &UnitBody, tick: u32) {
        m.allow_start(false);
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
            &Default::default(),
        );
    }
    fn finish(m: &mut Motion, w: &World, b: &UnitBody) -> (u32, u32) {
        let mut stopped_turns = 0;
        for tick in 0..3000 {
            let head = m.drive.head;
            step(m, w, b, tick);
            assert!(
                m.drive.vel <= Fx::from_int(m.speed),
                "speed limit increased"
            );
            assert!(!m.blocked, "a clear rounded route hit terrain");
            if m.drive.head != head && m.drive.vel <= Fx::ONE {
                stopped_turns += 1;
            }
            if m.destination.is_none() {
                return (tick + 1, stopped_turns);
            }
        }
        panic!(
            "route did not finish: {:?} {:?} cursor={}",
            m.pos, m.drive, m.cursor
        );
    }

    #[test]
    fn checked_corner_brakes_for_continuation_but_final_point_still_stops() {
        let at = [Fx::from_int(280), Fx::from_int(180)];
        let corner = [Fx::from_int(20), Fx::ZERO];
        assert_eq!(
            braking_distance(at, corner, Some([300, 460]), true),
            Fx::from_int(28)
        );
        assert_eq!(
            braking_distance(at, corner, Some([300, 460]), false),
            Fx::from_int(20)
        );
        assert_eq!(braking_distance(at, corner, None, true), Fx::from_int(20));
        assert_eq!(
            braking_distance(
                [Fx::from_int(300), Fx::from_int(455)],
                [Fx::ZERO, Fx::from_int(5)],
                None,
                false
            ),
            Fx::from_int(5)
        );
    }

    #[test]
    fn open_corners_finish_sooner_without_raising_speed_or_stopping_to_turn() {
        let w = world();
        for (kind, body) in [
            (RangeUnitKind::Tank, voxel::unit::tank()),
            (RangeUnitKind::Artillery, voxel::unit::artillery()),
            (RangeUnitKind::Strider, voxel::unit::strider()),
            (RangeUnitKind::Spider, voxel::unit::spider()),
        ] {
            for path in [
                vec![[300, 180], [300, 460]],
                vec![[300, 180], [480, 360]],
                vec![[260, 180], [300, 220], [340, 180], [500, 180]],
            ] {
                let mut flowing = motion(kind, &body, path);
                let mut pivoting = flowing.clone();
                pivoting.navigation.corner_lookahead_units = 0;
                let before = finish(&mut pivoting, &w, &body);
                let after = finish(&mut flowing, &w, &body);
                assert!(after.0 < before.0, "{kind:?}: {before:?} -> {after:?}");
                assert!(
                    after.1 <= before.1,
                    "{kind:?} rounded turn introduced stops: {before:?} -> {after:?}"
                );
            }
        }
    }

    #[test]
    fn nearby_unit_outside_turn_corridor_does_not_cancel_rolling_turn() {
        let w = world();
        let body = voxel::unit::tank();
        let mut m = motion(RangeUnitKind::Tank, &body, vec![[300, 180], [300, 460]]);
        m.pos[0] = Fx::from_int(280);
        m.drive.vel = Fx::from_int(20);
        let neighbour = Obstacle { height: Fx::ZERO,
            moving: false,
            pos: [Fx::from_int(245), Fx::from_int(16), Fx::from_int(230)],
            head: 0,
            half: [Fx::from_int(8); 2],
        };
        assert!(
            m.moving_steering(
                &w,
                &body,
                &[neighbour],
                &nav::vehicle::Rubble::new(),
                0,
                Fx::ONE
            )
            .is_some(),
            "a neighbour away from the actual turn should not force a pivot"
        );
    }

    #[test]
    fn close_tank_column_turns_on_the_move_without_excess_overlap() {
        let w = world();
        let body = voxel::unit::tank();
        let mut group: Vec<_> = (0..3)
            .map(|i| {
                let z = 140 + i * 80;
                let x = 300 + i * 80;
                let mut m = motion(RangeUnitKind::Tank, &body, vec![[x, z], [x, z + 200]]);
                m.pos[2] = Fx::from_int(z);
                m
            })
            .collect();
        let mut moving_turn = [false; 3];
        let mut stopped_in_turn = [0; 3];
        let rubble = nav::vehicle::Rubble::new();
        for tick in 0..600 {
            let traffic: Vec<_> = group.iter().map(|m| Traffic::of(m, &body, true)).collect();
            for id in 0..group.len() {
                let others: Vec<_> = group
                    .iter()
                    .enumerate()
                    .filter(|(j, _)| *j != id)
                    .map(|(_, m)| Obstacle::of(m, &body))
                    .collect();
                let m = &mut group[id];
                let head = m.drive.head;
                m.set_traffic_hint(m.traffic_hint(
                    &body,
                    id,
                    &traffic,
                    crate::mobility::Mobility::intact(true),
                ));
                m.allow_start(false);
                m.step(
                    &w,
                    &body,
                    true,
                    true,
                    crate::mobility::Mobility::intact(true),
                    &others,
                    tick,
                    98,
                    &rubble,
                    &Default::default(),
                );
                moving_turn[id] |= m.drive.head != head && m.drive.vel > Fx::ONE;
                let angle = angle::diff(m.drive.head, 0).abs();
                if angle > angle::FULL / 36 && angle < angle::FULL * 2 / 9 && m.drive.vel <= Fx::ONE
                {
                    stopped_in_turn[id] += 1;
                }
                assert!(m.drive.vel.abs() <= Fx::from_int(m.speed));
                for other in &others {
                    assert!(Obstacle::of(m, &body).penetration(other) <= HULL_CONTACT_SLOP);
                }
            }
            if group.iter().all(|m| m.destination.is_none()) {
                break;
            }
        }
        assert!(
            group.iter().all(|m| m.destination.is_none()),
            "group did not finish its turn"
        );
        assert!(
            moving_turn.into_iter().all(|v| v),
            "some tanks did not turn while moving"
        );
        assert!(
            stopped_in_turn.into_iter().all(|v| v <= 2),
            "tanks stopped during the turn: {stopped_in_turn:?}"
        );
    }

    #[test]
    fn a_single_voxel_surface_variation_keeps_rolling_preview() {
        struct ShallowStep;
        impl voxel::Generator for ShallowStep {
            fn material(&self, x: u32, y: u32, _: u32) -> u16 {
                if y < 16 || (x >= 304 && y == 16) {
                    2
                } else {
                    0
                }
            }
        }
        let w = World::new(640, 64, 640, 100000, Box::new(ShallowStep));
        w.prepare_navigation([4, 5]);
        let body = voxel::unit::tank();
        let mut m = motion(RangeUnitKind::Tank, &body, vec![[300, 180], [300, 460]]);
        m.pos[0] = Fx::from_int(280);
        m.drive.vel = Fx::from_int(20);
        assert!(m
            .moving_steering(&w, &body, &[], &nav::vehicle::Rubble::new(), 0, Fx::ONE)
            .is_some());
    }

    #[test]
    fn modest_course_correction_keeps_existing_forward_momentum() {
        let w = world();
        let body = voxel::unit::tank();
        let mut m = motion(RangeUnitKind::Tank, &body, vec![[300, 250]]);
        m.navigation.corner_lookahead_units = 0;
        m.drive.vel = Fx::from_int(20);
        let before = m.pos;
        step(&mut m, &w, &body, 0);
        assert!(!m.blocked);
        assert!(m.pos[0] > before[0]);
        assert!(
            m.drive.vel > Fx::from_int(20),
            "small turn caused braking: {:?}",
            m.drive
        );
        assert!(m.drive.vel <= Fx::from_int(m.speed));
    }

    #[test]
    fn turn_preview_rejects_live_craters_walls_debris_and_parked_hulls() {
        let mut w = world();
        let body = voxel::unit::tank();
        let mut m = motion(RangeUnitKind::Tank, &body, vec![[300, 180], [300, 460]]);
        m.pos[0] = Fx::from_int(280);
        m.drive.vel = Fx::from_int(20);
        let mut debris = nav::vehicle::Rubble::new();
        assert!(m
            .moving_steering(&w, &body, &[], &debris, 0, Fx::ONE)
            .is_some());
        let parked = Obstacle { height: Fx::ZERO,
            moving: false,
            pos: [Fx::from_int(300), Fx::from_int(16), Fx::from_int(210)],
            head: 0,
            half: [Fx::from_int(8); 2],
        };
        assert!(m
            .moving_steering(&w, &body, &[parked], &debris, 0, Fx::ONE)
            .is_none());
        debris.insert([284, 200], 35);
        assert!(m
            .moving_steering(&w, &body, &[], &debris, 0, Fx::ONE)
            .is_none());
        debris.remove(&[284, 200]);
        w.queue_edit(voxel::Edit::put(284, 16, 200, 2));
        w.flush();
        assert!(m
            .moving_steering(&w, &body, &[], &debris, 0, Fx::ONE)
            .is_none());
        w.prepare_navigation([4, 5]);
        assert!(m
            .moving_steering(&w, &body, &[], &debris, 0, Fx::ONE)
            .is_none());
        w.queue_edit(voxel::Edit::put(284, 16, 200, 0));
        w.flush();
        w.prepare_navigation([4, 5]);
        assert!(m
            .moving_steering(&w, &body, &[], &debris, 0, Fx::ONE)
            .is_some());
        for y in 0..16 {
            w.queue_edit(voxel::Edit::put(284, y, 200, 0));
        }
        w.flush();
        assert!(m
            .moving_steering(&w, &body, &[], &debris, 0, Fx::ONE)
            .is_none());
        w.prepare_navigation([4, 5]);
        assert!(m
            .moving_steering(&w, &body, &[], &debris, 0, Fx::ONE)
            .is_none());
    }

    #[test]
    fn crushable_forest_keeps_moving_turns_but_not_hard_obstacles() {
        struct Forest;
        impl voxel::Generator for Forest {
            fn material(&self, x: u32, y: u32, z: u32) -> u16 {
                if y < 16 {
                    2
                } else if (200..380).contains(&x) && (100..400).contains(&z) && y < 40 {
                    4
                } else {
                    0
                }
            }
        }
        for (kind, body) in [
            (RangeUnitKind::Tank, voxel::unit::tank()),
            (RangeUnitKind::Artillery, voxel::unit::artillery()),
            (RangeUnitKind::Strider, voxel::unit::strider()),
            (RangeUnitKind::Spider, voxel::unit::spider()),
        ] {
            let mut w = World::new(640, 64, 640, 100000, Box::new(Forest));
            w.prepare_navigation([4, 5]);
            let mut flowing = motion(kind, &body, vec![[300, 180], [300, 460]]);
            let mut pivoting = flowing.clone();
            pivoting.navigation.corner_lookahead_units = 0;
            let before = finish(&mut pivoting, &w, &body);
            let after = finish(&mut flowing, &w, &body);
            assert!(after.0 < before.0, "{kind:?}: {before:?}->{after:?}");
            assert!(after.1 < before.1, "forest still forces a standing turn");
            let mut m = motion(kind, &body, vec![[300, 180], [300, 460]]);
            m.pos[0] = Fx::from_int(280);
            m.drive.vel = Fx::from_int(20);
            let rubble = nav::vehicle::Rubble::new();
            assert!(m
                .moving_steering(&w, &body, &[], &rubble, 0, Fx::ONE)
                .is_some());
            w.queue_edit(voxel::Edit::put(284, 16, 200, 3));
            w.flush();
            assert!(m
                .moving_steering(&w, &body, &[], &rubble, 0, Fx::ONE)
                .is_none());
            w.prepare_navigation([4, 5]);
            assert!(m
                .moving_steering(&w, &body, &[], &rubble, 0, Fx::ONE)
                .is_none());
        }
    }

    #[test]
    fn reversal_keeps_its_pivot_and_a_mid_turn_snapshot_replays_exactly() {
        let w = world();
        let b = voxel::unit::tank();
        let mut a = motion(RangeUnitKind::Tank, &b, vec![[300, 180], [150, 180]]);
        for tick in 0..600 {
            step(&mut a, &w, &b, tick);
            if a.cursor > 0 {
                assert!(a.pos[0].sub(Fx::from_int(300)).abs() <= Fx::ONE);
                assert!(a.pos[2].sub(Fx::from_int(180)).abs() <= Fx::ONE);
                break;
            }
        }
        assert!(a.cursor > 0, "reversal never reached its checked pivot");
        let mut a = motion(RangeUnitKind::Tank, &b, vec![[300, 180], [300, 460]]);
        let mut tick = 0;
        while a.drive.head == 0 {
            step(&mut a, &w, &b, tick);
            tick += 1;
            assert!(tick < 300);
        }
        assert!(a.drive.vel > Fx::ONE);
        let mut saved = Blob::new();
        a.save(&mut saved);
        let mut restored = Motion::new([100, 16, 180], RangeUnitKind::Tank, &b);
        assert!(restored.load(&mut Scan::new(&saved.v)));
        for t in tick..tick + 400 {
            step(&mut a, &w, &b, t);
            step(&mut restored, &w, &b, t);
            let mut left = Blob::new();
            let mut right = Blob::new();
            a.save(&mut left);
            restored.save(&mut right);
            assert_eq!(left.v, right.v, "diverged at {t}");
        }
        assert!(a.destination.is_none());
    }

    #[test]
    fn right_hand_turn_wraps_heading_and_reaches_the_goal() {
        let w = world();
        for (kind, body) in [
            (RangeUnitKind::Tank, voxel::unit::tank()),
            (RangeUnitKind::Strider, voxel::unit::strider()),
            (RangeUnitKind::Spider, voxel::unit::spider()),
        ] {
            let mut m = motion(kind, &body, vec![[300, 320], [300, 100]]);
            m.pos[2] = Fx::from_int(320);
            let mut old = m.clone();
            old.navigation.corner_lookahead_units = 0;
            assert!(finish(&mut m, &w, &body).0 < finish(&mut old, &w, &body).0);
        }
    }
}

/// A rounded waypoint has a verified continuation. Allow a short distance
/// beyond the preview, capped by the conservative bound to the next point.
pub(super) fn braking_distance(
    position: [Fx; 2],
    delta: [Fx; 2],
    next: Option<[i32; 2]>,
    rounded: bool,
) -> Fx {
    let preview = delta[0].abs().max(delta[1].abs());
    if !rounded {
        return preview;
    }
    next.map_or(preview, |p| {
        let next = Fx::from_int(p[0])
            .sub(position[0])
            .abs()
            .max(Fx::from_int(p[1]).sub(position[1]).abs());
        next.min(preview.add(Fx::from_int(8)))
    })
}

/// Conservative capsule around a preview leg. Only a neighbour close to
/// the actual leg can force a stop; the broad bounding rectangle alone is
/// far too large for a column of units driving in parallel.
fn near_preview_leg(p: [Fx; 2], a: [Fx; 2], b: [Fx; 2], radius: Fx) -> bool {
    let d = [b[0].sub(a[0]), b[1].sub(a[1])];
    let v = [p[0].sub(a[0]), p[1].sub(a[1])];
    let length2 = d[0].mul(d[0]).add(d[1].mul(d[1]));
    let t = if length2 <= Fx::ZERO {
        Fx::ZERO
    } else {
        v[0].mul(d[0])
            .add(v[1].mul(d[1]))
            .div(length2)
            .max(Fx::ZERO)
            .min(Fx::ONE)
    };
    let dx = v[0].sub(d[0].mul(t));
    let dz = v[1].sub(d[1].mul(t));
    dx.mul(dx).add(dz.mul(dz)) <= radius.mul(radius)
}

impl Motion {
    pub(super) fn moving_steering(
        &self,
        world: &World,
        body: &UnitBody,
        others: &[Obstacle],
        rubble: &nav::vehicle::Rubble,
        straight_way: i32,
        straight_alignment: Fx,
    ) -> Option<Steering> {
        let preview = Fx::from_int(self.navigation.corner_lookahead_units);
        if preview <= Fx::ZERO || self.drive.vel < Fx::ZERO || self.blocked {
            return None;
        }
        let p = [self.pos[0], self.pos[2]];
        let corner = self.path[self.cursor].map(Fx::from_int);
        let to = [0, 1].map(|a| corner[a].sub(p[a]));
        let extent = |d: [Fx; 2]| d[0].abs().max(d[1].abs());
        if (extent(to) > preview || self.cursor + 1 == self.path.len())
            && straight_alignment > Fx::from_ratio(self.alignment, 100)
        {
            return None;
        }
        let forward = [angle::cos(self.drive.head), angle::sin(self.drive.head)];
        let dot = |a: [Fx; 2], b: [Fx; 2]| a[0].mul(b[0]).add(a[1].mul(b[1]));
        let mut aim = corner;
        let mut passed = false;
        let mut rounded = false;
        if extent(to) <= preview {
            if let Some(next) = self.path.get(self.cursor + 1) {
                let next = next.map(Fx::from_int);
                let leg = [0, 1].map(|a| next[a].sub(corner[a]));
                let length = extent(leg);
                // Reversals retain their checked pivot. A rounded corner is
                // consumed only once it falls behind the moving hull.
                if length > Fx::ZERO && dot(leg, forward) >= Fx::ZERO {
                    passed = dot(to, forward) <= Fx::ZERO
                        && dot([0, 1].map(|a| next[a].sub(p[a])), forward) > Fx::ZERO;
                    let lead = preview.min(length.div(Fx::from_int(2)));
                    aim = [0, 1].map(|a| corner[a].add(leg[a].mul(lead.div(length))));
                    rounded = !passed;
                }
            }
        }
        let delta = [0, 1].map(|a| aim[a].sub(p[a]));
        let length = extent(delta);
        if length <= Fx::ZERO {
            return None;
        }
        let way = if rounded || passed {
            angle::atan2(delta[1].raw(), delta[0].raw())
        } else {
            straight_way
        };
        let alignment = if rounded || passed {
            angle::cos(angle::diff(way, self.drive.head))
        } else {
            straight_alignment
        };
        if !rounded && !passed && alignment > Fx::from_ratio(self.alignment, 100) {
            return None;
        }
        if alignment <= Fx::from_ratio(self.navigation.moving_turn_alignment_pct, 100) {
            return None;
        }
        let lead = preview.min(length).div(length);
        let end = if rounded || passed {
            aim
        } else {
            [0, 1].map(|a| p[a].add(delta[a].mul(lead)))
        };
        let shape = self.shape(body);
        let reach = shape.half[0] + shape.half[1] + self.navigation.plan_margin;
        let coast = [0, 1].map(|a| p[a].add(forward[a].mul(preview)));
        let lo = [0, 1].map(|a| {
            let low = p[a].min(end[a]).min(coast[a]);
            (if rounded || passed {
                low.min(corner[a])
            } else {
                low
            })
            .to_int_trunc()
                - reach
        });
        let hi = [0, 1].map(|a| {
            let high = p[a].max(end[a]).max(coast[a]);
            (if rounded || passed {
                high.max(corner[a])
            } else {
                high
            })
            .to_int_trunc()
                + 1
                + reach
        });
        if (0..2)
            .any(|a| lo[a] + reach < EDGE || hi[a] - reach >= [world.nx, world.nz][a] as i32 - EDGE)
        {
            return None;
        }
        // A uniform non-yielding floor proves support and hard clearance
        // through the entire turn. Crushable trees do not require a pivot;
        // the normal live pose still prices/cuts vegetation on every step.
        if (0..2).any(|a| lo[a] < 0 || hi[a] >= [world.nx, world.nz][a] as i32) {
            return None;
        }
        // A uniform tile may lie 10 cm above its neighbour. A mixed tile
        // still forces the exact route because it may contain an obstacle.
        let mut floor: Option<[u16; 2]> = None;
        for z in lo[1] as u32 / 8..=hi[1] as u32 / 8 {
            for x in lo[0] as u32 / 8..=hi[0] as u32 / 8 {
                let [low, high] =
                    world.prepared_floor_tile(x, z, self.navigation.fell.materials)?;
                let [min, max] = floor.map_or([low, high], |[a, b]| [a.min(low), b.max(high)]);
                if low != high || max.saturating_sub(min) > 1 {
                    return None;
                }
                floor = Some([min, max]);
            }
        }
        if rubble.touches(lo, hi) {
            return None;
        }
        let own = Obstacle::of(self, body);
        let radius = |half: [Fx; 2]| half[0].mul(half[0]).add(half[1].mul(half[1])).sqrt();
        let own_radius = radius(own.half).add(Fx::from_int(self.navigation.plan_margin));
        if others.iter().any(|o| {
            let broad_radius = o.half[0].add(o.half[1]);
            if !(0..2).all(|a| {
                let center = o.pos[a * 2];
                center.add(broad_radius) >= Fx::from_int(lo[a])
                    && center.sub(broad_radius) <= Fx::from_int(hi[a])
            }) {
                return false;
            }
            let center = [o.pos[0], o.pos[2]];
            let clearance = own_radius.add(radius(o.half));
            near_preview_leg(center, p, end, clearance)
                || near_preview_leg(center, p, coast, clearance)
                || ((rounded || passed)
                    && (near_preview_leg(center, p, corner, clearance)
                        || near_preview_leg(center, corner, end, clearance)))
        }) {
            return None;
        }
        Some(Steering {
            delta,
            way,
            alignment,
            passed,
            rounded,
        })
    }
}
