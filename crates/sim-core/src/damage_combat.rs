//! Spatial combat adapter. Rendering never supplies a hit or a damage mask.
use crate::{
    damage::{self, Bodies, Frame},
    index::Grid,
    Units, NO_TARGET,
};
use fx::Fx;
use phys::V3;

#[derive(Clone, Copy)]
pub(crate) struct Hit {
    pub unit: u32,
    pub t: Fx,
    pub frame: Frame,
    pub layers: [u8; 2],
}

/// Swept broad phase, then the occupied volume. Stable distance/ID tie break.
pub(crate) fn trace(
    bodies: &Bodies,
    frames: &[Option<[Frame; 2]>],
    units: &Units,
    grid: &Grid,
    from: V3,
    to: V3,
    ignore: u32,
) -> Option<Hit> {
    let r = bodies.max_radius();
    let lo = [
        from.x.min(to.x).to_int_trunc() - r,
        from.z.min(to.z).to_int_trunc() - r,
    ];
    let hi = [
        from.x.max(to.x).to_int_trunc() + r,
        from.z.max(to.z).to_int_trunc() + r,
    ];
    let mut best: Option<Hit> = None;
    grid.for_each_rect(lo, hi, |id| {
        if id == ignore || !units.alive[id as usize] {
            return;
        }
        let Some(frames) = frames[id as usize] else {
            return;
        };
        let body = bodies.body(id).unwrap();
        let ty = body.turret_y;
        let layers = if ty == 0 {
            [[0, body.vol.ny], [0, 0]]
        } else {
            [[0, ty], [ty, body.vol.ny]]
        };
        for (frame, layers) in frames.into_iter().zip(layers) {
            if layers[0] == layers[1] {
                continue;
            }
            let Some((t, _)) =
                damage::first_hit_layers(body, frame.local(from), frame.local(to), layers)
            else {
                continue;
            };
            if best.is_none_or(|old| t < old.t || (t == old.t && id < old.unit)) {
                best = Some(Hit {
                    unit: id,
                    t,
                    frame,
                    layers,
                });
            }
        }
    });
    best
}

/// Continue into the struck body along the incident direction, even when the
/// simulation step ended inside the first layer. The material work limits depth.
pub(crate) fn queue(
    bodies: &mut Bodies,
    hit: Hit,
    from: V3,
    to: V3,
    energy_mj: u64,
    max_pending: u32,
    motion_pct: u32,
) -> bool {
    let a = hit.frame.local(from);
    let b = hit.frame.local(to);
    let d: [Fx; 3] = std::array::from_fn(|i| b[i].sub(a[i]));
    let longest = d.iter().map(|x| x.abs()).max().unwrap_or(Fx::ZERO);
    if longest == Fx::ZERO {
        return false;
    }
    let body = bodies.body(hit.unit).unwrap();
    let span = Fx::from_int(body.vol.nx as i32 + body.vol.ny as i32 + body.vol.nz as i32);
    let extra = span.div(longest);
    let end = std::array::from_fn(|i| b[i].add(d[i].mul(extra)));
    let motion_mj = (energy_mj as u128 * motion_pct as u128 / 100) as u64;
    if !bodies.queue(
        hit.unit,
        a,
        end,
        energy_mj - motion_mj,
        max_pending as usize,
    ) {
        return false;
    }
    let request = bodies.pending.back_mut().unwrap();
    request.direction = to.sub(from);
    request.motion_mj = motion_mj;
    bodies
        .pending
        .back_mut()
        .unwrap()
        .ray
        .cells
        .retain(|c| c[1] >= hit.layers[0] && c[1] < hit.layers[1]);
    true
}

pub(crate) fn point(p: (i32, i32, i32)) -> V3 {
    V3::new(Fx::from_int(p.0), Fx::from_int(p.1), Fx::from_int(p.2))
}
pub(crate) fn cell(p: V3) -> (i32, i32, i32) {
    (p.x.to_int_trunc(), p.y.to_int_trunc(), p.z.to_int_trunc())
}

pub(crate) fn cell_entry(from: V3, to: V3, at: (i32, i32, i32)) -> Fx {
    let a = [from.x, from.y, from.z];
    let d = [to.x.sub(from.x), to.y.sub(from.y), to.z.sub(from.z)];
    let p = [at.0, at.1, at.2];
    let mut enter = Fx::ZERO;
    for i in 0..3 {
        if d[i] == Fx::ZERO {
            continue;
        }
        enter = enter.max(
            Fx::from_int(p[i] + i32::from(d[i] < Fx::ZERO))
                .sub(a[i])
                .div(d[i]),
        );
    }
    enter
}

impl crate::World {
    pub(crate) fn apply_material_damage(&mut self) {
        let Some(bodies) = self.material_bodies.as_mut() else {
            return;
        };
        let t = &self.tuning.impact;
        let frames = bodies.frames(&self.units);
        // Old jobs cannot export matter after death moved/released the container.
        bodies.pending.retain(|r| {
            self.units
                .alive
                .get(r.unit as usize)
                .copied()
                .unwrap_or(false)
        });
        let room = if self.econ.is_some() {
            self.bursts.fragment_room(&self.tuning.blast)
        } else {
            t.cells_per_tick
        };
        let changes = bodies.step(&t.work_kj_m3, t.cells_per_tick.min(room), t.jobs_per_tick);
        // `voxels` is retained for the old benchmark and construction counters.
        // Completed material units die by core connectivity, not that counter.
        for k in 0..self.units.live_ids.len() {
            let id = self.units.live_ids[k];
            if self.units.site[id as usize] != 0 {
                continue;
            }
            let Some(s) = bodies.state(id) else {
                continue;
            };
            if !s.status.alive {
                self.units.voxels[id as usize] = 0;
            }
            if !s.status.sensor {
                self.units.target[id as usize] = NO_TARGET;
            }
        }
        self.export_body_damage(changes, &frames);
        if self.econ.is_some() {
            let eligible: Vec<bool> = (0..self.units.len())
                .map(|i| {
                    self.units.alive[i]
                        && self.units.site[i] == 0
                        && self.units.store[i] != crate::economy::NO_CONTAINER
                        && self.units.kind[i] != crate::economy::NO_KIND
                })
                .collect();
            let t = &self.tuning.impact;
            let sections = self.material_bodies.as_mut().unwrap().step_detach(
                t.detach_work_per_tick,
                t.detach_jobs_per_tick,
                self.bursts.fragment_room(&self.tuning.blast),
                &eligible,
            );
            self.export_sections(sections, &frames);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::World;
    use voxel::{Part, UnitBody};

    fn fixture() -> World {
        let mut tuning = crate::Tuning::from_ron(include_str!("../../../data/sim.ron")).unwrap();
        tuning.gravity = 0;
        tuning.impact.work_kj_m3 = [1; 5];
        tuning.impact.motion_pct = 0;
        let mut w = World::new(2, 16, tuning);
        w.attach_weapons(modding::weapons::from_ron("(weapons:[(id:1,kind:Bolt,speed:1800,damage:99,reload:30,blast_radius:0,blast_damage:0,mass_g:0,impact_energy_j:1)])").unwrap());
        let mut body = UnitBody::new_scaled(8, 3, 3, 1, 100);
        for x in 0..8 {
            body.set(x, 1, 1, Part::Armour);
        }
        body.set(0, 1, 1, Part::Engine);
        body.set(7, 1, 1, Part::Core);
        assert!(body.seat_core());
        let mut bodies = Bodies::new(vec![body]);
        for (x, z) in [(40, 40), (70, 40)] {
            let id = w.units.spawn(x, z, 1, 30, 99);
            w.units.y[id as usize] = 10;
            bodies.spawn_at(id, 0);
        }
        w.material_bodies = Some(bodies);
        w.units.refresh_live();
        w.grid_far
            .rebuild(&w.units.x, &w.units.z, &w.units.live_ids);
        w
    }
    fn shot(w: &mut World, x: i32, y: i32, vx: i32) -> u32 {
        let id = w.shots.spawn(
            (x, y, 40),
            (Fx::ZERO, Fx::from_ratio(1, 2), Fx::ZERO),
            (Fx::from_int(vx), Fx::ZERO, Fx::ZERO),
            0,
            99,
            10,
            (0, 0),
        );
        w.shots.weapon_id[id as usize] = 1;
        w.shots.refresh_live();
        id
    }
    #[test]
    fn allocating_motion_does_not_overflow_the_largest_energy_dose() {
        let mut w = fixture();
        let bodies = w.material_bodies.as_mut().unwrap();
        let frame = bodies.frames(&w.units)[0].unwrap()[0];
        let hit = Hit {
            unit: 0,
            t: Fx::ZERO,
            frame,
            layers: [0, 3],
        };
        assert!(queue(
            bodies,
            hit,
            point((20, 11, 40)),
            point((60, 11, 40)),
            u64::MAX,
            16,
            10
        ));
        let r = bodies.pending.back().unwrap();
        assert_eq!(r.motion_mj, u64::MAX / 10);
        assert_eq!(r.ray.initial_mj, u64::MAX - r.motion_mj);
    }
    #[test]
    fn a_fast_bolt_hits_the_real_engine_and_spares_the_next_unit() {
        let mut w = fixture();
        shot(&mut w, 20, 11, 60);
        w.step_shots();
        assert_eq!(w.shots_hit, 1);
        w.apply_material_damage();
        let bodies = w.material_bodies.as_ref().unwrap();
        assert_eq!(bodies.state(0).unwrap().removed, [[0, 1, 1]]);
        assert!(bodies.state(0).unwrap().status.alive);
        assert!(!bodies.state(0).unwrap().status.engine);
        assert_eq!(
            w.units.voxels[0], 99,
            "material damage must not subtract scalar HP"
        );
        assert_eq!(bodies.state(1).unwrap().revision, 0);
        // A move order still exists; failed propulsion must override it.
        w.delta.resize(w.units.len(), crate::drive::Want::default());
        w.delta[0].way = 0;
        w.apply_deltas();
        assert_eq!(w.units.vel[0], Fx::ZERO);
        assert_eq!(w.units.x[0], 40);
    }
    #[test]
    fn an_empty_layer_does_not_hit_a_cylinder_around_the_unit() {
        let mut w = fixture();
        let id = shot(&mut w, 20, 12, 60);
        w.step_shots();
        w.apply_material_damage();
        assert_eq!(w.shots_hit, 0);
        assert!(w.shots.alive[id as usize]);
        assert_eq!(
            w.material_bodies
                .as_ref()
                .unwrap()
                .state(0)
                .unwrap()
                .revision,
            0
        );
    }
    #[test]
    fn a_core_hit_kills_without_draining_the_legacy_counter() {
        let mut w = fixture();
        // Reverse shot meets the core first. The engine end is untouched.
        shot(&mut w, 55, 11, -25);
        w.step_shots();
        w.apply_material_damage();
        assert!(
            !w.material_bodies
                .as_ref()
                .unwrap()
                .state(0)
                .unwrap()
                .status
                .alive
        );
        assert_eq!(w.units.voxels[0], 0);
        w.phase_cleanup();
        assert!(!w.units.alive[0]);
        assert!(w.units.alive[1]);
    }
    #[test]
    fn reverse_flight_hits_the_near_wall_and_does_not_damage_the_target() {
        struct Walls;
        impl voxel::Generator for Walls {
            fn material(&self, x: u32, _y: u32, _z: u32) -> voxel::store::Mat {
                if x == 50 || x == 60 {
                    2
                } else {
                    0
                }
            }
        }
        let mut w = fixture();
        let ground = voxel::World::new(128, 64, 128, 65536, Box::new(Walls));
        w.attach_ground(crate::ground::Ground::new(ground, &w.tuning));
        // Remove the unit at x=70 from the shot's path.
        w.units.kill(1);
        w.units.refresh_live();
        w.grid_far
            .rebuild(&w.units.x, &w.units.z, &w.units.live_ids);
        let id = shot(&mut w, 80, 11, -60);
        w.step_shots();
        w.apply_material_damage();
        assert_eq!(w.shots.x[id as usize], 60);
        assert_eq!(w.shots_hit, 0);
        assert_eq!(
            w.material_bodies
                .as_ref()
                .unwrap()
                .state(0)
                .unwrap()
                .revision,
            0
        );
    }
    #[test]
    fn a_full_damage_queue_defers_the_projectile_without_losing_its_step() {
        let mut w = fixture();
        w.tuning.impact.max_pending = 1;
        let bodies = w.material_bodies.as_mut().unwrap();
        bodies.queue(0, [Fx::ZERO; 3], [Fx::ONE; 3], 1000, 1);
        let id = shot(&mut w, 20, 11, 60) as usize;
        w.step_shots();
        assert_eq!((w.shots.x[id], w.shots.life[id]), (20, 10));
        w.apply_material_damage();
        w.step_shots();
        assert_eq!(w.shots_hit, 1);
    }
    #[test]
    fn turret_collision_follows_aim_while_the_hull_keeps_its_heading() {
        let mut w = fixture();
        let mut body = UnitBody::new_scaled(8, 6, 3, 1, 100);
        body.set(7, 4, 1, Part::Weapon);
        body.turret_y = 3;
        let mut bodies = Bodies::new(vec![body]);
        bodies.spawn_at(0, 0);
        w.units.z[1] = 60;
        w.units.x[1] = 40;
        w.units.target[0] = 1;
        let frames = bodies.frames(&w.units);
        let turret = frames[0].unwrap()[1];
        let c = turret.world([
            Fx::from_ratio(15, 2),
            Fx::from_ratio(9, 2),
            Fx::from_ratio(3, 2),
        ]);
        let a = c.sub(V3::new(Fx::from_int(10), Fx::ZERO, Fx::ZERO));
        let b = c.add(V3::new(Fx::from_int(10), Fx::ZERO, Fx::ZERO));
        let hit = trace(&bodies, &frames, &w.units, &w.grid_far, a, b, NO_TARGET).unwrap();
        assert_eq!(hit.unit, 0);
        assert_eq!(hit.layers, [3, 6]);
    }
    #[test]
    fn leaving_the_world_does_not_skip_a_hit_before_its_edge() {
        struct Empty;
        impl voxel::Generator for Empty {
            fn material(&self, _x: u32, _y: u32, _z: u32) -> voxel::store::Mat {
                0
            }
        }
        let mut w = fixture();
        w.attach_ground(crate::ground::Ground::new(
            voxel::World::new(128, 64, 128, 65536, Box::new(Empty)),
            &w.tuning,
        ));
        w.grid_far
            .rebuild(&w.units.x, &w.units.z, &w.units.live_ids);
        shot(&mut w, 20, 11, 200);
        w.step_shots();
        w.apply_material_damage();
        assert_eq!(w.shots_hit, 1);
        assert_eq!(
            w.material_bodies
                .as_ref()
                .unwrap()
                .state(0)
                .unwrap()
                .removed,
            [[0, 1, 1]]
        );
    }
    #[test]
    fn a_new_unit_in_the_shooters_slot_is_not_immune_to_its_old_projectile() {
        let mut w = fixture();
        let id = shot(&mut w, 20, 11, 60) as usize;
        w.shots.source[id] = 0;
        w.shots.source_generation[id] = 1;
        let bodies = w.material_bodies.as_mut().unwrap();
        bodies.despawn(0);
        bodies.spawn_at(0, 0);
        w.step_shots();
        w.apply_material_damage();
        assert_eq!(
            w.material_bodies
                .as_ref()
                .unwrap()
                .state(0)
                .unwrap()
                .removed,
            [[0, 1, 1]]
        );
    }
    #[test]
    fn an_unarmed_kind_cannot_fire_the_benchmark_weapon_in_slot_zero() {
        let mut w = fixture();
        let kinds = modding::kinds::from_ron(
            include_str!("../../../data/kinds.ron"),
            crate::TICKS_PER_SECOND as u32,
        )
        .unwrap();
        let mats =
            modding::data::materials_from_ron(include_str!("../../../data/materials.ron")).unwrap();
        w.attach_economy(kinds, mats);
        let crawler = w.spawn_kind(20, 40, 0, 2).unwrap() as usize;
        w.units.target[crawler] = 0;
        w.units.refresh_live();
        w.fire();
        assert_eq!(w.shots_fired, 0);
        assert_eq!(w.beams_fired, 0);
    }
}
