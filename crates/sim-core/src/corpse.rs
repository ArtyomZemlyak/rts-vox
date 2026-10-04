//! A dead unit keeps its immutable damaged hull and the existing wreck matter.
//! Its durable record is independent of the small-fragment capacity and unit ID.
use super::*;
use crate::damage::Frame;
impl crate::World {
    pub(crate) fn remember_corpse(&mut self, id: usize, held: u32, frames: Option<[Frame; 2]>) {
        if self.units.site[id] != 0 {
            return;
        }
        let Some(frames) = frames else {
            return;
        };
        let Some(bodies) = &self.material_bodies else {
            return;
        };
        let Some(state) = bodies.state(id as u32) else {
            return;
        };
        let count = state.shape.count();
        if count == 0 {
            return;
        }
        let source = bodies.shared_body(id as u32).unwrap();
        let mm = source.mm;
        let chunk = chunk::Chunk::from_body(
            state.template,
            self.units.owner[id],
            source,
            frames,
            state.shape.clone(),
        )
        .with_dents(state.dents.clone());
        let divisor = Fx::from_int(crate::TICKS_PER_SECOND);
        let speed = self.units.vel[id].div(divisor);
        let (hc, hs) = crate::ride::heading_of(self.units.head[id]);
        let velocity = V3::new(
            hc.mul(speed),
            self.units.body_vy(id).div(divisor),
            hs.mul(speed),
        );
        let vmax = Fx::from_ratio(
            self.tuning.blast.max_speed_mm_s as i32,
            voxel::unit::WORLD_MM as i32 * crate::TICKS_PER_SECOND,
        );
        let speed2 = velocity.len_sq();
        let velocity = if speed2 > vmax.mul(vmax) {
            velocity.scale(vmax.div(speed2.sqrt()))
        } else {
            velocity
        };
        let density = self.tuning.chassis(self.units.chassis[id]).density.max(1) as u64;
        self.bursts.next_id += 1;
        let mut f = Fragment {
            id: self.bursts.next_id,
            position: chunk.centre,
            velocity,
            material: AIR,
            paint: Some([self.units.owner[id], 0, 0]),
            mm,
            basis: IDENTITY,
            mass_g: (density * (mm as u64).pow(3) / 1_000_000).max(1) * count as u64,
            count,
            shape: 1,
            side: 2,
            chunk: Some(Box::new(chunk)),
            held,
            resting: false,
        };
        // A hull dead at the map edge hangs over it (ADR-428).
        if let Some(g) = &self.ground {
            f.keep_on_map([g.world.nx, g.world.nz]);
        }
        assert!(self.bursts.corpses.insert(held, f).is_none());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (crate::World, u32) {
        let t = crate::Tuning::from_ron(include_str!("../../../data/sim.ron")).unwrap();
        let mut w = crate::World::new(42, 16, t);
        let kinds = modding::kinds::from_ron(include_str!("../../../data/kinds.ron"), 30).unwrap();
        let mats =
            modding::data::materials_from_ron(include_str!("../../../data/materials.ron")).unwrap();
        w.attach_economy(kinds, mats);
        let id = w.spawn_kind(60, 60, 0, 1).unwrap();
        w.units.refresh_live();
        w.units.y[id as usize] = 40;
        (w, id)
    }
    #[test]
    fn a_dead_hull_keeps_the_damage_velocity_and_one_existing_matter_container() {
        let (mut w, id) = fixture();
        let before = w
            .econ
            .as_ref()
            .unwrap()
            .ledger
            .held(w.units.store[id as usize]);
        let b = w.material_bodies.as_mut().unwrap();
        let body = b.body(id).unwrap();
        let mut core = Vec::new();
        for z in 0..body.vol.nz {
            for y in 0..body.vol.ny {
                for x in 0..body.vol.nx {
                    if body.vol.get(x as usize, y as usize, z as usize)
                        && body.part_at(x as usize, y as usize, z as usize) == voxel::Part::Core
                    {
                        core.push([x, y, z]);
                    }
                }
            }
        }
        let generation = b.generations[id as usize];
        for batch in core.chunks(768) {
            b.queue_cells(
                id,
                generation,
                batch.to_vec(),
                u64::MAX / 8,
                0,
                V3::ZERO,
                w.tuning.impact.max_pending,
            );
        }
        w.apply_material_damage();
        assert!(
            !w.material_bodies
                .as_ref()
                .unwrap()
                .state(id)
                .unwrap()
                .status
                .alive
        );
        let body = w.material_bodies.as_ref().unwrap().shared_body(id).unwrap();
        w.units.vel[id as usize] = Fx::from_int(90);
        w.units.heave_v[id as usize] = Fx::from_int(-60);
        w.phase_cleanup();
        assert!(!w.units.alive[id as usize]);
        assert_eq!(w.bursts.corpses.len(), 1);
        let f = w.bursts.corpses.values().next().unwrap();
        assert_eq!(f.count, body.solid_count());
        assert_eq!(f.velocity.x, Fx::from_int(3));
        assert_eq!(f.velocity.y, Fx::from_int(-2));
        assert_eq!(
            f.chunk.as_ref().unwrap().body.as_ref().unwrap().vol.cols,
            body.vol.cols
        );
        let e = w.econ.as_ref().unwrap();
        assert_eq!(e.wrecks.iter().filter(|v| v.held == f.held).count(), 1);
        let matter = e.ledger.held(f.held)
            + w.bursts
                .fragments
                .iter()
                .map(|f| e.ledger.held(f.held))
                .sum::<u64>();
        assert_eq!(matter + e.sunk(matter::Sink::ContainerBreach), before);
        assert!(e.balanced());
    }
    #[test]
    fn reusing_the_unit_and_reclaiming_the_corpse_cannot_reanimate_old_geometry() {
        let (mut w, id) = fixture();
        w.units.voxels[id as usize] = 0;
        w.phase_cleanup();
        let held = *w.bursts.corpses.keys().next().unwrap();
        let old = w.bursts.corpses[&held].chunk.as_ref().unwrap().hash();
        let new = w.spawn_kind(140, 120, 1, 3).unwrap();
        assert_eq!(new, id);
        assert_eq!(old, w.bursts.corpses[&held].chunk.as_ref().unwrap().hash());
        let e = w.econ.as_mut().unwrap();
        let to = e.ledger.create(matter::Kind::Hauler, 0, u64::MAX);
        let n = e.ledger.held(held);
        assert_eq!(e.ledger.transfer(held, to, n), n);
        w.sweep_wrecks();
        assert!(w.bursts.corpses.is_empty());
        assert!(w.econ.as_ref().unwrap().wrecks.is_empty());
        let e = w.econ.as_mut().unwrap();
        let reused = e.ledger.create(matter::Kind::Debris, 0, u64::MAX);
        assert_eq!(reused, held);
        assert!(w.bursts.corpses.is_empty());
        assert!(e.balanced());
    }
    #[test]
    fn corpse_storage_does_not_depend_on_space_in_the_small_fragment_pool() {
        let (mut w, id) = fixture();
        w.tuning.blast.max_fragments = 0;
        w.tuning.blast.max_flying = 0;
        assert_eq!(w.bursts.fragment_room(&w.tuning.blast), 0);
        w.units.voxels[id as usize] = 0;
        w.phase_cleanup();
        assert!(!w.units.alive[id as usize]);
        assert_eq!(w.bursts.corpses.len(), 1);
        assert!(w.econ.as_ref().unwrap().balanced());
    }
}
