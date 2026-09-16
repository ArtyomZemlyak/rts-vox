//! Transfer actually removed unit cells into neutral physical debris.
//! The body's container pays each cumulative volume share once; no reveal/sink.
use super::*;
use crate::damage::{Change, Frame};

impl crate::World {
    pub(crate) fn export_body_damage(
        &mut self,
        changes: Vec<Change>,
        frames: &[Option<[Frame; 2]>],
    ) {
        let (Some(e), Some(bodies)) = (self.econ.as_mut(), self.material_bodies.as_mut()) else {
            return;
        };
        for change in changes {
            let id = change.unit as usize;
            let held = self.units.store[id];
            let ki = self.units.kind[id];
            if change.removed.is_empty()
                || held == crate::economy::NO_CONTAINER
                || ki == crate::economy::NO_KIND
                || self.units.site[id] != 0
            {
                continue;
            }
            let Some(frames) = frames[id] else {
                continue;
            };
            let state = bodies.slots[id].as_mut().unwrap();
            let template = &bodies.templates[state.template as usize];
            let cost = e.kinds.at(ki as usize).cost;
            let volume = template.volume_mm3().max(1);
            let cell_volume = (template.mm as u64).pow(3);
            let mut groups: BTreeMap<([u8; 3], [u8; 3], usize), u8> = BTreeMap::new();
            for p in &change.removed {
                let [x, y, z] = p.map(|v| v as usize);
                let origin = p.map(|v| v / 2 * 2);
                let sub = (p[0] - origin[0]) + 2 * (p[1] - origin[1]) + 4 * (p[2] - origin[2]);
                let paint = [
                    self.units.owner[id],
                    template.finish_at(x, y, z),
                    template.part_at(x, y, z) as u8,
                ];
                let layer = usize::from(template.turret_y > 0 && p[1] >= template.turret_y);
                *groups.entry((origin, paint, layer)).or_default() |= 1 << sub;
            }
            let density = self.tuning.chassis(self.units.chassis[id]).density.max(1) as u64;
            for ((origin, paint, layer), whole) in groups {
                for shape in shape_components(u64::from(whole), 2) {
                    let count = shape.count_ones();
                    let before = (cost as u128 * state.debris_mm3 as u128 / volume as u128) as u64;
                    state.debris_mm3 += count as u64 * cell_volume;
                    debug_assert!(state.debris_mm3 <= state.removed_mm3);
                    let after = (cost as u128 * state.debris_mm3 as u128 / volume as u128) as u64;
                    let amount = after - before;
                    let container = e.ledger.create(matter::Kind::Debris, 0, u64::MAX);
                    let moved = e.ledger.transfer(held, container, amount);
                    assert_eq!(
                        moved, amount,
                        "removed body volume must still be in its container"
                    );
                    let cap = e.ledger.get(held).cap;
                    assert_eq!(e.ledger.lower_cap(held, cap.saturating_sub(amount)), 0);
                    let frame = frames[layer];
                    let local = arr(shape_centre(shape, 2));
                    let position = frame.world(std::array::from_fn(|a| {
                        Fx::from_int(origin[a] as i32)
                            .add(Fx::from_ratio(1, 2))
                            .add(local[a])
                    }));
                    let f = Fragment {
                        id: 0,
                        position,
                        velocity: V3::ZERO,
                        material: AIR,
                        paint: Some(paint),
                        mm: template.mm,
                        basis: [frame.x, frame.y, frame.z],
                        mass_g: (density * cell_volume / 1_000_000).max(1) * count as u64,
                        count,
                        shape,
                        side: 2,
                        chunk: None,
                        held: container,
                        resting: false,
                    };
                    let energy = (change.motion_mj as u128 * count as u128
                        / (change.removed.len() + change.dented.len()).max(1) as u128)
                        as u64;
                    self.bursts
                        .emit_body_fragment(f, change.direction, energy, &self.tuning.blast);
                }
            }
        }
    }
}

impl crate::World {
    pub(crate) fn export_sections(
        &mut self,
        sections: Vec<crate::damage::Separated>,
        frames: &[Option<[Frame; 2]>],
    ) {
        let (Some(e), Some(bodies)) = (self.econ.as_mut(), self.material_bodies.as_mut()) else {
            return;
        };
        for part in sections {
            let id = part.unit as usize;
            let state = bodies.slots[id].as_mut().unwrap();
            let template = &bodies.templates[state.template as usize];
            let count = part.cells.len() as u32;
            let volume = template.volume_mm3().max(1);
            let cell_volume = (template.mm as u64).pow(3);
            let ki = self.units.kind[id];
            let held = self.units.store[id];
            assert!(
                ki != crate::economy::NO_KIND
                    && held != crate::economy::NO_CONTAINER
                    && self.units.site[id] == 0
            );
            let cost = e.kinds.at(ki as usize).cost;
            let before = (cost as u128 * state.debris_mm3 as u128 / volume as u128) as u64;
            state.debris_mm3 += count as u64 * cell_volume;
            assert!(state.debris_mm3 <= state.removed_mm3);
            let after = (cost as u128 * state.debris_mm3 as u128 / volume as u128) as u64;
            let container = e.ledger.create(matter::Kind::Debris, 0, u64::MAX);
            assert_eq!(
                e.ledger.transfer(held, container, after - before),
                after - before
            );
            let cap = e.ledger.get(held).cap;
            assert_eq!(
                e.ledger.lower_cap(held, cap.saturating_sub(after - before)),
                0
            );
            let frames = frames[id].unwrap();
            let chunk = chunk::Chunk::new(
                state.template,
                self.units.owner[id],
                part.cells,
                frames,
                part.shape,
            );
            let speed = self.units.vel[id].div(Fx::from_int(crate::TICKS_PER_SECOND));
            let vmax = Fx::from_ratio(
                self.tuning.blast.max_speed_mm_s as i32,
                voxel::unit::WORLD_MM as i32 * crate::TICKS_PER_SECOND,
            );
            let (hc, hs) = crate::ride::heading_of(self.units.head[id]);
            let velocity = V3::new(
                hc.mul(speed),
                self.units
                    .body_vy(id)
                    .div(Fx::from_int(crate::TICKS_PER_SECOND)),
                hs.mul(speed),
            );
            let speed2 = velocity.len_sq();
            let velocity = if speed2 > vmax.mul(vmax) {
                velocity.scale(vmax.div(speed2.sqrt()))
            } else {
                velocity
            };
            let density = self.tuning.chassis(self.units.chassis[id]).density.max(1) as u64;
            let f = Fragment {
                id: 0,
                position: chunk.centre,
                velocity,
                material: AIR,
                paint: Some([self.units.owner[id], 0, 0]),
                mm: template.mm,
                basis: IDENTITY,
                mass_g: (density * cell_volume / 1_000_000).max(1) * count as u64,
                count,
                shape: 1,
                side: 2,
                chunk: Some(chunk),
                held: container,
                resting: false,
            };
            // Inherited velocity belongs to the separated mass. Detachment adds
            // no launch energy; gravity does the subsequent work.
            self.bursts.emit_section(f);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (crate::World, u32, Vec<voxel::impact::Cell>) {
        let t = crate::Tuning::from_ron(include_str!("../../../data/sim.ron")).unwrap();
        let mut w = crate::World::new(7, 16, t);
        let kinds = modding::kinds::from_ron(include_str!("../../../data/kinds.ron"), 30).unwrap();
        let mats =
            modding::data::materials_from_ron(include_str!("../../../data/materials.ron")).unwrap();
        w.attach_economy(kinds, mats);
        let id = w.spawn_kind(40, 40, 0, 2).unwrap();
        w.units.y[id as usize] = 20;
        w.units.refresh_live();
        let body = w.material_bodies.as_ref().unwrap().body(id).unwrap();
        let mut cells = Vec::new();
        'scan: for z in 0..body.vol.nz {
            for y in 0..body.vol.ny {
                for x in 0..body.vol.nx {
                    if body.vol.get(x as usize, y as usize, z as usize)
                        && body.part_at(x as usize, y as usize, z as usize) == voxel::Part::Armour
                    {
                        cells.push([x, y, z]);
                        if cells.len() == 3 {
                            break 'scan;
                        }
                    }
                }
            }
        }
        (w, id, cells)
    }
    #[test]
    fn removed_armour_transfers_its_mass_and_matter_without_turning_cargo_into_body() {
        let (mut w, id, cells) = fixture();
        let held = w.units.store[id as usize];
        let e = w.econ.as_mut().unwrap();
        let cargo = e.ledger.create(matter::Kind::Hauler, 1234, u64::MAX);
        assert_eq!(e.ledger.transfer(cargo, held, 1234), 1234);
        let before = e.ledger.held(held);
        let before_cap = e.ledger.get(held).cap;
        let bodies = w.material_bodies.as_mut().unwrap();
        let state = bodies.state(id).unwrap();
        let generation = state.generation;
        let mm = bodies.body(id).unwrap().mm as u64;
        let volume = mm.pow(3) * cells.len() as u64;
        let fracture = w.tuning.impact.work_kj_m3[0] as u64 * volume / 1000;
        let motion = 1_000_000;
        assert!(bodies.queue_cells(
            id,
            generation,
            cells.clone(),
            fracture,
            motion,
            IDENTITY[0],
            w.tuning.impact.max_pending
        ));
        w.apply_material_damage();
        let e = w.econ.as_ref().unwrap();
        let debris = w
            .bursts
            .fragments
            .iter()
            .map(|f| e.ledger.held(f.held))
            .sum::<u64>();
        assert!(debris > 0);
        assert_eq!(e.ledger.held(held) + debris, before);
        assert_eq!(e.ledger.get(held).cap + debris, before_cap);
        assert_eq!(w.cargo(id as usize), 1234);
        assert_eq!(w.bursts.body_removed, 3);
        assert!(w.bursts.body_motion_mj > 0 && w.bursts.body_motion_mj <= motion);
        assert!(w
            .bursts
            .fragments
            .iter()
            .all(|f| f.paint.is_some() && f.mm as u64 == mm));
        let expected = (w.tuning.chassis(w.units.chassis[id as usize]).density as u64 * mm.pow(3)
            / 1_000_000)
            .max(1)
            * cells.len() as u64;
        assert_eq!(
            w.bursts.fragments.iter().map(|f| f.mass_g).sum::<u64>(),
            expected
        );
        assert!(e.balanced());
        assert_eq!(
            w.material_bodies
                .as_ref()
                .unwrap()
                .state(id)
                .unwrap()
                .debris_mm3,
            volume
        );
    }
    #[test]
    fn a_full_fragment_pool_defers_material_removal_until_it_has_room() {
        let (mut w, id, cells) = fixture();
        w.tuning.blast.max_flying = 1;
        let bodies = w.material_bodies.as_mut().unwrap();
        let generation = bodies.state(id).unwrap().generation;
        bodies.queue_cells(
            id,
            generation,
            cells,
            9_000_000_000,
            10000,
            IDENTITY[0],
            w.tuning.impact.max_pending,
        );
        w.apply_material_damage();
        assert_eq!(w.bursts.body_removed, 1);
        assert!(!w.material_bodies.as_ref().unwrap().pending.is_empty());
        let hash = w.material_bodies.as_ref().unwrap().hash();
        w.apply_material_damage();
        assert_eq!(hash, w.material_bodies.as_ref().unwrap().hash());
        w.tuning.blast.max_flying = 4;
        w.apply_material_damage();
        assert_eq!(w.bursts.body_removed, 3);
        assert!(w.material_bodies.as_ref().unwrap().pending.is_empty());
        assert!(w.econ.as_ref().unwrap().balanced());
    }
}
