//! Range control reads the command-phase snapshot. It never integrates motion:
//! NO_WAY lets the existing mass/thrust/brake model slow the machine down.
use crate::{World, NO_TARGET, STANCE_ATTACK_MOVE, STANCE_ENGAGED};
impl World {
    pub(crate) fn update_engagements(&mut self) {
        let u = &mut self.units;
        for &id in &u.live_ids {
            let i = id as usize;
            if !matches!(u.stance[i], STANCE_ATTACK_MOVE | STANCE_ENGAGED) {
                continue;
            }
            let target = u.target[i] as usize;
            let functional = self
                .material_bodies
                .as_ref()
                .and_then(|b| b.state(id))
                .is_none_or(|s| s.status.alive && s.status.weapon && s.status.sensor);
            let armed = self.econ.as_ref().is_none_or(|e| {
                u.kind[i] == crate::economy::NO_KIND || e.kinds.at(u.kind[i] as usize).weapon != 0
            });
            if target >= u.len()
                || u.target[i] == NO_TARGET
                || !u.alive[target]
                || u.owner[i] == u.owner[target]
                || !functional
                || !armed
            {
                u.stance[i] = STANCE_ATTACK_MOVE;
                continue;
            }
            let range = if self.weapons.is_empty() {
                self.tuning.range
            } else {
                let w = self
                    .weapons
                    .at((u.weapon[i] as usize).min(self.weapons.len() - 1));
                if w.range > 0 {
                    w.range
                } else {
                    self.tuning.range
                }
            };
            let band = usize::from(u.stance[i] == STANCE_ENGAGED);
            let limit = range as i64 * self.tuning.engagement_pct[band] as i64 / 100;
            let dx = u.x[target] as i64 - u.x[i] as i64;
            let dy = u.body_cell_y(target) as i64 - u.body_cell_y(i) as i64;
            let dz = u.z[target] as i64 - u.z[i] as i64;
            u.stance[i] = if dx * dx + dy * dy + dz * dz <= limit * limit {
                STANCE_ENGAGED
            } else {
                STANCE_ATTACK_MOVE
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Order, STANCE_MOVE};
    use fx::Fx;
    fn fixture() -> World {
        let mut t = crate::Tuning::from_ron(include_str!("../../../data/sim.ron")).unwrap();
        t.range = 1000;
        t.engagement_pct = [80, 90];
        let mut w = World::new(13, 16, t);
        for (x, owner) in [(100, 0), (890, 1)] {
            w.units.spawn(x, 100, owner, 100, 9999);
        }
        w.units.refresh_live();
        w.apply_order(Order::Hold { unit: 1 });
        w.units.target[0] = 1;
        w.apply_order(Order::AttackMove {
            unit: 0,
            x: 2000,
            z: 100,
        });
        w
    }
    #[test]
    fn contact_has_a_range_band_and_does_not_cancel_the_destination() {
        let mut w = fixture();
        w.update_engagements();
        assert_eq!(w.units.stance[0], STANCE_ENGAGED);
        w.units.x[1] = 950;
        w.update_engagements();
        assert_eq!(w.units.stance[0], STANCE_ENGAGED);
        w.units.x[1] = 1001;
        w.update_engagements();
        assert_eq!(w.units.stance[0], STANCE_ATTACK_MOVE);
        w.units.x[1] = 950;
        w.update_engagements();
        assert_eq!(w.units.stance[0], STANCE_ATTACK_MOVE);
        w.units.x[1] = 890;
        w.update_engagements();
        assert_eq!(w.units.stance[0], STANCE_ENGAGED);
        w.units.alive[1] = false;
        w.update_engagements();
        assert_eq!(w.units.stance[0], STANCE_ATTACK_MOVE);
        assert_eq!(w.units.goal_x[0], 2000);
        w.units.alive[1] = true;
        w.apply_order(Order::MoveTo {
            unit: 0,
            x: 2000,
            z: 100,
        });
        w.update_engagements();
        assert_eq!(w.units.stance[0], STANCE_MOVE);
    }
    #[test]
    fn an_engagement_brakes_with_inertia_and_worker_slices_agree() {
        let mut w = fixture();
        w.units.vel[0] = Fx::from_int(3);
        w.update_engagements();
        w.grid_near
            .rebuild(&w.units.x, &w.units.z, &w.units.live_ids);
        w.movement_deltas(1);
        let one = w.delta.clone();
        w.movement_deltas(4);
        assert_eq!(one, w.delta);
        assert_eq!(w.movement_kernel(0), w.delta[0]);
        let speed = w.units.vel[0];
        w.apply_deltas();
        assert!(w.units.vel[0] > Fx::ZERO && w.units.vel[0] < speed);
        for _ in 0..120 {
            w.update_engagements();
            w.movement_deltas(1);
            w.apply_deltas();
        }
        assert_eq!(w.units.vel[0], Fx::ZERO);
        assert!(w.units.x[1] - w.units.x[0] > 700);
    }
    #[test]
    fn the_equipped_weapon_overrides_the_fallback_engagement_range() {
        let mut w = fixture();
        w.attach_weapons(
            modding::weapons::from_ron(
                "(weapons:[(id:1,kind:Laser,speed:0,damage:1,reload:30,range:500,blast_radius:0,blast_damage:0)])",
            )
            .unwrap(),
        );
        w.update_engagements();
        assert_eq!(w.units.stance[0], STANCE_ATTACK_MOVE);
        w.units.x[1] = 450;
        w.update_engagements();
        assert_eq!(w.units.stance[0], STANCE_ENGAGED);
    }
}
