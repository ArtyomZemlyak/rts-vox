//! Atomic grouping of actually removed body cells. No extra damage and no
//! welding across units, articulated parts, poses or subsystem boundaries.
use super::*;
use crate::burst::{fracture_box, shape_centre, shape_components};
use crate::damage::Frame;
use std::collections::BTreeMap;

type Key = (usize, usize, [u32; 3], u8, [i64; 16]);
type PanelKey = (usize, usize, u8, u8, u8, [i64; 16]);
type PanelCells = (Frame, u16, u8, Vec<(voxel::impact::Cell, usize)>);
#[derive(Default)]
pub(super) struct Batch {
    groups: BTreeMap<Key, (Frame, Vec<(u32, usize)>)>,
    panels: BTreeMap<PanelKey, PanelCells>,
    span_mm: u32,
    // Conservative legacy component reservations. Panels may join them;
    // aligned partitions never split them into additional physical objects.
    components: BTreeMap<(Key, u8), Vec<u64>>,
    pub(super) room: Option<u32>,
}
impl Batch {
    pub(super) fn with_room(room: Option<u32>) -> Self {
        Self {
            room,
            ..Self::default()
        }
    }
    #[allow(clippy::too_many_arguments)]
    fn location(
        unit: usize,
        part: usize,
        cell: [usize; 3],
        frame: Frame,
        side: u8,
        tuning: &modding::tuning::Blast,
    ) -> (Key, u32) {
        let p = cell.map(|v| v as u32);
        let (origin, side) = fracture_box(p, side, tuning);
        let s = u32::from(side);
        let bit = p[0] - origin[0] + s * (p[1] - origin[1]) + s * s * (p[2] - origin[2]);
        let mut pose = [0; 16];
        let mut n = 0;
        for v in [frame.base, frame.x, frame.y, frame.z] {
            for a in [v.x, v.y, v.z] {
                pose[n] = a.raw();
                n += 1;
            }
        }
        for a in frame.centre {
            pose[n] = a.raw();
            n += 1;
        }
        pose[n] = frame.scale.raw();
        ((unit, part, origin, side, pose), bit)
    }
    /// Reserve before transferring matter. A face-connected addition needs no
    /// new piece; a bridge returns the reservations of the components it joins.
    /// No geometry is regrouped or published until the normal atomic commit.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn reserve(
        &mut self,
        unit: usize,
        part: usize,
        cell: [usize; 3],
        frame: Frame,
        side: u8,
        role: u8,
        tuning: &modding::tuning::Blast,
    ) -> bool {
        let (key, bit) = Self::location(unit, part, cell, frame, side, tuning);
        let side = u32::from(key.3);
        let xyz = [bit % side, bit / side % side, bit / (side * side)];
        let mut neighbours = 0;
        for (axis, stride) in [1, side, side * side].into_iter().enumerate() {
            if xyz[axis] > 0 {
                neighbours |= 1u64 << (bit - stride);
            }
            if xyz[axis] + 1 < side {
                neighbours |= 1u64 << (bit + stride);
            }
        }
        let pieces = self.components.entry((key, role)).or_default();
        // A caller must actually remove the previous cell before claiming it
        // again. A duplicate cannot manufacture geometry or release a slot.
        if pieces.iter().any(|shape| shape & (1u64 << bit) != 0) {
            return false;
        }
        let joined = pieces
            .iter()
            .filter(|shape| **shape & neighbours != 0)
            .count();
        let room = self
            .room
            .as_mut()
            .expect("component admission needs a cache");
        if joined == 0 && *room == 0 {
            return false;
        }
        if joined == 0 {
            *room -= 1;
        } else {
            *room += (joined - 1) as u32;
        }
        let mut shape = 1u64 << bit;
        pieces.retain(|piece| {
            if *piece & neighbours == 0 {
                true
            } else {
                shape |= *piece;
                false
            }
        });
        pieces.push(shape);
        true
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) fn record(
        &mut self,
        index: usize,
        unit: usize,
        part: usize,
        cell: [usize; 3],
        frame: Frame,
        side: u8,
        tuning: &modding::tuning::Blast,
        body: &voxel::unit::UnitBody,
    ) {
        let layer = u8::from(body.turret_y > 0 && cell[1] >= usize::from(body.turret_y));
        let (key, bit) = Self::location(
            unit,
            part * 2 + usize::from(layer),
            cell,
            frame,
            side,
            tuning,
        );
        if self.span_mm > 0 {
            self.panels
                .entry((
                    unit,
                    part,
                    layer,
                    body.part_at(cell[0], cell[1], cell[2]) as u8,
                    side,
                    key.4,
                ))
                .or_insert_with(|| (frame, body.mm, body.turret_y, Vec::new()))
                .3
                .push((cell.map(|v| v as u8), index));
            return;
        }
        self.groups
            .entry(key)
            .or_insert_with(|| (frame, Vec::new()))
            .1
            .push((bit, index));
    }
}

fn motion_mj(f: &Fragment) -> u64 {
    let mm_s = [f.velocity.x, f.velocity.y, f.velocity.z].map(|v| {
        (v.raw() as i128 * voxel::unit::WORLD_MM as i128 * crate::TICKS_PER_SECOND as i128
            / (1i128 << 32)) as i64
    });
    voxel::impact::kinetic_mj(f.mass_g.min(u32::MAX as u64) as u32, mm_s)
}

impl Matter {
    pub(crate) fn begin_body_batch(&mut self) {
        let room = self.claim_room();
        self.begin_body_batch_with_room(room);
    }
    /// Consecutive atomic batches in the same serial phase can reuse the
    /// exact remaining component/terrain reservation count. Never across ticks.
    pub(crate) fn begin_body_batch_with_room(&mut self, room: u32) {
        assert!(self.batch.is_none());
        let mut batch = Batch::with_room(Some(room));
        batch.span_mm = self.body_fragment_mm;
        self.batch = Some(batch);
    }
    /// Publish the exact remaining capacity after joining reserved components.
    /// The caller carries this value to the next wave/job in the same phase.
    pub(crate) fn finish_body_batch(&mut self) -> u32 {
        let Some(batch) = self.batch.take() else {
            return 0;
        };
        let room = batch.room;
        if batch.span_mm > 0 {
            let reserved: usize = batch.components.values().map(Vec::len).sum();
            let pieces = self.finish_panels(batch);
            return room.map_or(0, |room| {
                room + u32::try_from(
                    reserved
                        .checked_sub(pieces)
                        .expect("aligned panels cannot split reserved components"),
                )
                .unwrap()
            });
        }
        let mut removed = BTreeSet::new();
        for ((_, _, origin, side, _), (frame, cells)) in batch.groups {
            // Keep subsystem boundaries, but paint scratches do not sever steel.
            let mut roles = BTreeMap::<u8, (u64, Vec<(u32, usize)>)>::new();
            for (bit, index) in cells {
                let role = self.debris.fragments[index].paint.unwrap()[2];
                let group = roles.entry(role).or_default();
                group.0 |= 1 << bit;
                group.1.push((bit, index));
            }
            for (_, (whole, cells)) in roles {
                for shape in shape_components(whole, side) {
                    let indices: Vec<_> = cells
                        .iter()
                        .filter(|(bit, _)| shape & (1 << bit) != 0)
                        .map(|(_, index)| *index)
                        .collect();
                    if indices.len() < 2 {
                        continue;
                    }
                    let first = indices[0];
                    let held = self.debris.fragments[first].held;
                    let mut velocity = V3::ZERO;
                    let mut mass = 0;
                    let mut old_motion = 0;
                    for &i in &indices {
                        let f = &self.debris.fragments[i];
                        velocity = velocity.add(f.velocity);
                        mass += f.mass_g;
                        old_motion += motion_mj(f);
                        if i != first {
                            let container = f.held;
                            let amount = self.economy.ledger.held(container);
                            assert_eq!(
                                self.economy.ledger.transfer(container, held, amount),
                                amount
                            );
                            assert!(self.economy.ledger.retire_empty_debris(container));
                            removed.insert(i);
                        }
                    }
                    let centre = shape_centre(shape, side);
                    let local = [centre.x, centre.y, centre.z];
                    let f = &mut self.debris.fragments[first];
                    f.position = frame.world(std::array::from_fn(|a| {
                        Fx::from_int(origin[a] as i32)
                            .add(Fx::from_ratio(1, 2))
                            .add(local[a])
                    }));
                    // Equal-volume cells of one material: mean velocity preserves
                    // momentum and cannot increase kinetic energy.
                    f.velocity = velocity.scale(Fx::from_ratio(1, indices.len() as i32));
                    f.mass_g = mass;
                    f.count = shape.count_ones();
                    f.shape = shape;
                    f.side = side;
                    let dissipated = old_motion.saturating_sub(motion_mj(f));
                    self.debris.body_motion_mj =
                        self.debris.body_motion_mj.saturating_sub(dissipated);
                }
            }
        }
        self.compact_body_donors(removed);
        room.unwrap_or(0)
    }
    fn finish_panels(&mut self, batch: Batch) -> usize {
        use crate::burst::body_fragments::{components, set_geometry, span_cells};
        let mut removed = BTreeSet::new();
        let mut count = 0;
        for ((unit, part, _, _, quantum, _), (frame, mm, turret_y, cells)) in batch.panels {
            for piece in components(cells, span_cells(mm, batch.span_mm, quantum)) {
                count += 1;
                if piece.len() < 2 {
                    continue;
                }
                let first = piece[0].1;
                let held = self.debris.fragments[first].held;
                let paint = self.debris.fragments[first].paint;
                let preserve_finishes = piece
                    .iter()
                    .any(|&(_, i)| self.debris.fragments[i].paint != paint);
                let mut velocity = V3::ZERO;
                let mut mass = 0;
                let mut old_motion = 0;
                for &(_, index) in &piece {
                    let f = &self.debris.fragments[index];
                    velocity = velocity.add(f.velocity);
                    mass += f.mass_g;
                    old_motion += motion_mj(f);
                    if index != first {
                        let container = f.held;
                        let amount = self.economy.ledger.held(container);
                        assert_eq!(
                            self.economy.ledger.transfer(container, held, amount),
                            amount
                        );
                        assert!(self.economy.ledger.retire_empty_debris(container));
                        removed.insert(index);
                    }
                }
                let f = &mut self.debris.fragments[first];
                f.velocity = velocity.scale(Fx::from_ratio(1, piece.len() as i32));
                f.mass_g = mass;
                set_geometry(
                    f,
                    unit as u16,
                    part.checked_sub(1).map(|p| p as u8),
                    frame,
                    piece.into_iter().map(|(cell, _)| cell).collect(),
                    turret_y,
                    preserve_finishes,
                );
                self.debris.body_motion_mj = self
                    .debris
                    .body_motion_mj
                    .saturating_sub(old_motion.saturating_sub(motion_mj(f)));
            }
        }
        self.compact_body_donors(removed);
        count
    }
    fn compact_body_donors(&mut self, removed: BTreeSet<usize>) {
        if removed.is_empty() {
            return;
        }
        // Only this batch's appended suffix can contain donors. Leave the
        // long-lived settled prefix untouched and preserve every survivor's
        // order (this is stable compaction, never swap-and-pop deletion).
        let first = *removed.first().unwrap();
        let mut removed = removed.into_iter().peekable();
        let fragments = &mut self.debris.fragments;
        let mut write = first;
        for read in first..fragments.len() {
            if removed.peek() == Some(&read) {
                removed.next();
            } else {
                if write != read {
                    fragments.swap(write, read);
                }
                write += 1;
            }
        }
        fragments.truncate(write);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reservations_follow_face_components_without_crossing_fracture_boundaries() {
        let tuning = crate::Tuning::from_ron(include_str!("../../../data/sim.ron")).unwrap();
        let frame = Frame {
            base: V3::ZERO,
            x: crate::burst::IDENTITY[0],
            y: crate::burst::IDENTITY[1],
            z: crate::burst::IDENTITY[2],
            centre: [Fx::ZERO; 3],
            scale: Fx::ONE,
        };
        let mut batch = Batch::with_room(Some(2));
        let reserve = |batch: &mut Batch, unit, part, cell, frame, role| {
            batch.reserve(unit, part, cell, frame, 4, role, &tuning.blast)
        };
        assert!(reserve(&mut batch, 0, 0, [0, 0, 0], frame, 0));
        assert!(reserve(&mut batch, 0, 0, [2, 0, 0], frame, 0));
        assert_eq!(batch.room, Some(0));
        // Touching a corner/edge, crossing a box, owner, part, role or exact
        // pose does not extend either reserved physical component.
        for (unit, part, cell, pose, role) in [
            (0, 0, [1, 1, 0], frame, 0),
            (0, 0, [4, 0, 0], frame, 0),
            (1, 0, [1, 0, 0], frame, 0),
            (0, 1, [1, 0, 0], frame, 0),
            (0, 0, [1, 0, 0], frame, 1),
            (
                0,
                0,
                [1, 0, 0],
                Frame {
                    base: V3::UP,
                    ..frame
                },
                0,
            ),
        ] {
            assert!(!reserve(&mut batch, unit, part, cell, pose, role));
            assert_eq!(batch.room, Some(0));
        }
        // This cell joins the two pieces: one slot returns immediately.
        assert!(reserve(&mut batch, 0, 0, [1, 0, 0], frame, 0));
        assert_eq!(batch.room, Some(1));
        assert!(!reserve(&mut batch, 0, 0, [1, 0, 0], frame, 0));
        assert_eq!(batch.room, Some(1));
        assert!(reserve(&mut batch, 0, 0, [1, 1, 0], frame, 1));
        assert_eq!(batch.room, Some(0));
    }

    #[test]
    fn reservations_match_final_components_for_varied_cell_orders() {
        let tuning = crate::Tuning::from_ron(include_str!("../../../data/sim.ron")).unwrap();
        let frame = Frame {
            base: V3::ZERO,
            x: crate::burst::IDENTITY[0],
            y: crate::burst::IDENTITY[1],
            z: crate::burst::IDENTITY[2],
            centre: [Fx::ZERO; 3],
            scale: Fx::ONE,
        };
        for side in 1..=4u8 {
            let n = u32::from(side);
            for stride in [1, 3, 5, 7, 11, 13, 17, 31, 63] {
                let mut batch = Batch::with_room(Some(64));
                let mut masks = BTreeMap::<Key, u64>::new();
                // Odd strides permute the 64 cells, exposing gaps and bridges.
                for k in 0..64 {
                    let bit = (k * stride) % 64;
                    let cell = [
                        (bit % 4) as usize,
                        (bit / 4 % 4) as usize,
                        (bit / 16) as usize,
                    ];
                    assert!(batch.reserve(0, 0, cell, frame, side, 0, &tuning.blast));
                    let (key, bit) = Batch::location(0, 0, cell, frame, side, &tuning.blast);
                    *masks.entry(key).or_default() |= 1u64 << bit;
                    let pieces = masks
                        .iter()
                        .map(|(key, &mask)| shape_components(mask, key.3).len())
                        .sum::<usize>();
                    assert_eq!(
                        batch.room,
                        Some(64 - pieces as u32),
                        "side={n}, stride={stride}, k={k}"
                    );
                }
            }
        }
    }

    #[test]
    fn component_admission_preserves_scalar_when_room_is_ample_and_respects_caps() {
        let config = modding::explosion::ExplosionConfig::from_ron(include_str!(
            "../../../data/energy-cannon.ron"
        ))
        .unwrap();
        let layout =
            modding::range::RangeLayout::from_ron(include_str!("../../../data/range-cover.ron"))
                .unwrap();
        for (total, flying, claims) in [(1, 1, 1), (31, 7, 3), (31, 31, 31), (4096, 4096, 4096)] {
            let run = |cached: bool| {
                let mut r = crate::blast::BlastRange::with_layout(config, layout.clone()).unwrap();
                let u = &r.units[0];
                let m = &mut r.matter;
                m.tuning.max_fragments = total;
                m.tuning.max_flying = flying;
                m.tuning.max_claims = claims;
                m.begin(u32::MAX, 4);
                m.begin_body_batch();
                if !cached {
                    m.batch.as_mut().unwrap().room = None;
                }
                // Duplicate claims, full claim budget, mixed terrain/body
                // reservations, insufficient energy and full flying/total caps.
                for x in [40, 40, 41, 42, 43] {
                    let _ = m.ground(&mut r.world, [x, 2, 40], [40, 2, 40]);
                    if cached {
                        assert_eq!(m.claim_room(), m.debris.fragment_room(&m.tuning));
                    }
                }
                let mut attempted = 0;
                for z in 0..usize::from(u.body.vol.nz) {
                    for y in 0..usize::from(u.body.vol.ny) {
                        for x in 0..usize::from(u.body.vol.nx) {
                            if !u.body.vol.get(x, y, z) || attempted >= 128 {
                                continue;
                            }
                            let cell = [x, y, z];
                            if attempted % 5 == 0 {
                                m.melt_unit(0, u, 0, cell);
                            } else {
                                m.body(0, u, cell, &u.root_frame(y), [200, 16, 330]);
                            }
                            attempted += 1;
                        }
                    }
                }
                m.remaining_mj = 0;
                let _ = m.body(0, u, [1, 1, 1], &u.root_frame(1), [200, 16, 330]);
                let profile = m.admission;
                m.finish_body_batch();
                // Claims hold slots per piece they will make, not per cell
                // (ADR-659); the claim ceiling still counts cells.
                assert!(m.debris.claims() <= claims as usize);
                assert!(m.debris.fragments.len() + m.debris.reserved() <= total as usize);
                assert!(
                    m.debris.fragments.iter().filter(|f| !f.resting).count() + m.debris.reserved()
                        <= flying as usize
                );
                assert!(m.economy.ledger.balanced());
                let mut bytes = fx::blob::Blob::new();
                m.save(&mut bytes);
                r.world.save(&mut bytes);
                (bytes.v, profile)
            };
            let (actual, fast) = run(true);
            let (expected, slow) = run(false);
            if total == 4096 {
                assert_eq!(actual, expected, "ample capacity preserves exact output");
                assert_eq!(fast.body, slow.body);
                assert_eq!(fast.melt, slow.melt);
            }
            assert!(fast.room_scans < slow.room_scans);
            assert!(fast.room_items <= slow.room_items);
            assert!(slow.room_scans > 100);
            if total == 4096 {
                assert_eq!(fast.room_scans, 1);
            }
        }
    }

    #[test]
    fn thermal_reservations_match_scalar_melts_with_caps_and_invalid_cells() {
        let config = modding::explosion::ExplosionConfig::from_ron(include_str!(
            "../../../data/energy-cannon.ron"
        ))
        .unwrap();
        let layout =
            modding::range::RangeLayout::from_ron(include_str!("../../../data/range-cover.ron"))
                .unwrap();
        for cap in [1, 7, 128] {
            let run = |reserved: bool| {
                let mut r = crate::blast::BlastRange::with_layout(config, layout.clone()).unwrap();
                let u = &r.units[0];
                let m = &mut r.matter;
                m.tuning.max_flying = cap;
                let mut room = m.debris.fragment_room(&m.tuning);
                let mut result = Vec::new();
                // Air must not consume a reservation.
                result.push(if reserved {
                    m.melt_unit_with_room(0, u, 0, [0, 0, 0], &mut room)
                } else {
                    m.melt_unit(0, u, 0, [0, 0, 0])
                });
                let mut n = 0;
                for z in 0..usize::from(u.body.vol.nz) {
                    for y in 0..usize::from(u.body.vol.ny) {
                        for x in 0..usize::from(u.body.vol.nx) {
                            if !u.body.vol.get(x, y, z) || n >= 128 {
                                continue;
                            }
                            result.push(if reserved {
                                m.melt_unit_with_room(0, u, 0, [x, y, z], &mut room)
                            } else {
                                m.melt_unit(0, u, 0, [x, y, z])
                            });
                            n += 1;
                            if reserved {
                                assert_eq!(room, m.debris.fragment_room(&m.tuning));
                            }
                        }
                    }
                }
                assert!(m.economy.ledger.balanced());
                let mut bytes = fx::blob::Blob::new();
                m.save(&mut bytes);
                (result, bytes.v, m.admission)
            };
            let (actual, fast, a) = run(true);
            let (expected, slow, b) = run(false);
            assert_eq!(actual, expected);
            assert_eq!(fast, slow);
            assert_eq!(a.melt, b.melt);
            assert_eq!(a.room_scans, 0);
            assert!(b.room_scans > 100);
        }
    }

    #[test]
    fn grouped_body_tail_preserves_settled_prefix_and_exact_removed_cells() {
        let config = modding::explosion::ExplosionConfig::from_ron(include_str!(
            "../../../data/energy-cannon.ron"
        ))
        .unwrap();
        let layout =
            modding::range::RangeLayout::from_ron(include_str!("../../../data/range-cover.ron"))
                .unwrap();
        let mut r = crate::blast::BlastRange::with_layout(config, layout).unwrap();
        let u = &r.units[0];
        let b = &u.body;
        let mut cells = None;
        'boxes: for z in (0..usize::from(b.vol.nz).saturating_sub(3)).step_by(4) {
            for y in (0..usize::from(b.vol.ny).saturating_sub(3)).step_by(4) {
                for x in (0..usize::from(b.vol.nx).saturating_sub(3)).step_by(4) {
                    let group: Vec<_> = (z..z + 4)
                        .flat_map(|z| {
                            (y..y + 4).flat_map(move |y| (x..x + 4).map(move |x| [x, y, z]))
                        })
                        .collect();
                    if group.iter().all(|&[x, y, z]| {
                        b.vol.get(x, y, z) && b.part_at(x, y, z) == voxel::Part::Armour
                    }) {
                        cells = Some(group);
                        break 'boxes;
                    }
                }
            }
        }
        let cells = cells.expect("fixture has a full armour fracture box");
        let m = &mut r.matter;
        // Four old settled pieces plus ONE free slot must admit the whole
        // connected 64-cell plate without changing its exact final geometry.
        m.tuning.max_fragments = 5;
        m.tuning.max_flying = 1;
        m.begin(u32::MAX, 4);
        // Existing settled pieces must keep their positions, shape, order
        // and ledger owners when only the new suffix is compacted.
        let mut prefix_cells = Vec::new();
        'prefix: for z in 0..usize::from(b.vol.nz) {
            for y in 0..usize::from(b.vol.ny) {
                for x in 0..usize::from(b.vol.nx) {
                    if !b.vol.get(x, y, z) || cells.contains(&[x, y, z]) {
                        continue;
                    }
                    assert!(matches!(
                        m.body(0, u, [x, y, z], &u.root_frame(y), [200, 16, 330]),
                        ClaimResult::Applied
                    ));
                    prefix_cells.push([x, y, z]);
                    let f = m.debris.fragments.last_mut().unwrap();
                    f.resting = true;
                    f.velocity = V3::ZERO;
                    if m.debris.fragments.len() == 4 {
                        break 'prefix;
                    }
                }
            }
        }
        let signature = |f: &Fragment| {
            (
                f.id, f.held, f.position, f.velocity, f.basis, f.shape, f.side, f.count, f.mass_g,
                f.resting,
            )
        };
        let prefix: Vec<_> = m.debris.fragments.iter().map(signature).collect();
        let mut expected_mass = m.debris.fragments.iter().map(|f| f.mass_g).sum::<u64>();
        m.begin_body_batch();
        let body_before = m.economy.ledger.held(m.body_container(0));
        for &cell in &cells {
            assert!(matches!(
                m.body(0, u, cell, &u.root_frame(cell[1]), [200, 16, 330]),
                ClaimResult::Applied
            ));
            let f = m.debris.fragments.last().unwrap();
            expected_mass += f.mass_g;
            assert_eq!(m.claim_room(), 0);
        }
        m.finish_body_batch();
        assert_eq!(
            m.debris.fragments[..4]
                .iter()
                .map(signature)
                .collect::<Vec<_>>(),
            prefix
        );
        assert_eq!(m.admission.body, [68, 0, 0]);
        assert_eq!(m.debris.fragments.len(), 5);
        assert_eq!(m.debris.fragments.iter().map(|f| f.count).sum::<u32>(), 68);
        let mut restored = BTreeSet::new();
        for f in &m.debris.fragments[4..] {
            if let Some(chunk) = &f.chunk {
                assert_eq!(
                    crate::burst::body_fragments::components(
                        chunk.cells.iter().copied().enumerate().map(|(i, c)| (c, i)),
                        256
                    )
                    .len(),
                    1
                );
                for cell in &chunk.cells {
                    assert!(restored.insert(cell.map(usize::from)));
                }
                continue;
            }
            let centre = shape_centre(f.shape, f.side);
            let scale = Fx::from_ratio(f.mm as i32, voxel::unit::WORLD_MM as i32);
            for bit in 0..u32::from(f.side).pow(3) {
                if f.shape & (1 << bit) == 0 {
                    continue;
                }
                let side = u32::from(f.side);
                let delta = [
                    Fx::from_int((bit % side) as i32).sub(centre.x),
                    Fx::from_int((bit / side % side) as i32).sub(centre.y),
                    Fx::from_int((bit / (side * side)) as i32).sub(centre.z),
                ];
                let mut world = f.position;
                for (axis, d) in f.basis.iter().zip(delta) {
                    world = world.add(axis.scale(d.mul(scale)));
                }
                let local = u
                    .root_frame(cells[0][1])
                    .local(world)
                    .map(|v| v.to_int_trunc() as usize);
                assert!(restored.insert(local));
            }
        }
        assert_eq!(restored, cells.iter().copied().collect());
        assert_eq!(
            m.debris.fragments.iter().map(|f| f.mass_g).sum::<u64>(),
            expected_mass
        );
        assert_eq!(
            body_before - m.economy.ledger.held(m.body_container(0)),
            64 * m.economy.per_voxel(3)
        );
        assert!(m.economy.ledger.balanced());
        assert!(m
            .debris
            .fragments
            .iter()
            .all(|f| shape_components(f.shape, f.side) == [f.shape]));
        // Commit the corresponding body removals, then round-trip even though
        // retired temporary donors outnumber the five final physical pieces.
        for [x, y, z] in prefix_cells.into_iter().chain(cells) {
            assert!(r.units[0].body.remove(x, y, z));
        }
        let mut bytes = Blob::new();
        m.save(&mut bytes);
        assert!(m.economy.ledger.snapshot().0.len() > 5 + r.units.len() * 2 + 2);
        let mut copy = Matter::new(&r.units);
        copy.tuning = m.tuning;
        assert!(copy
            .load(&mut Scan::new(&bytes.v), &r.world, &r.units)
            .is_some());
        let mut restored = Blob::new();
        copy.save(&mut restored);
        assert_eq!(bytes.v, restored.v);
    }

    #[test]
    fn panels_of_all_unit_kinds_and_articulated_parts_keep_their_source_and_roundtrip() {
        use modding::range::RangeUnitKind;
        let mut units: Vec<_> = RangeUnitKind::ALL
            .into_iter()
            .map(|kind| {
                // Large factory/storage templates must also fit the world
                // used by the saved-fragment bounds validator below.
                let mut unit = RangeUnit::of_kind(kind, [1200, 128, 1200]);
                unit.motion.drive.head = fx::angle::FULL / 7;
                unit.turret_pitch = fx::angle::FULL / 24;
                unit.update_pose(-fx::angle::FULL / 11);
                unit
            })
            .collect();
        let mut matter = Matter::new(&units);
        // Exercise geometry/source restoration across every tier, rather than
        // exhausting one ordinary weapon's u32-joule payload on T3 gun armour.
        matter.begin_mj(u64::MAX / 1024, 4);
        matter.begin_body_batch();
        let mut removed = Vec::new();
        for (id, u) in units.iter().enumerate() {
            for part in 0..=u.body.segments.len() {
                let b = if part == 0 {
                    &u.body
                } else {
                    &u.body.segments[part - 1].body
                };
                let mut count = [0; 2];
                for z in 0..usize::from(b.vol.nz) {
                    for y in 0..usize::from(b.vol.ny) {
                        for x in 0..usize::from(b.vol.nx) {
                            let layer = usize::from(b.turret_y > 0 && y >= usize::from(b.turret_y));
                            if !b.vol.get(x, y, z) || count[layer] == 128 {
                                continue;
                            }
                            let frame = if part == 0 {
                                u.root_frame(y)
                            } else {
                                u.seg_frames[part - 1]
                            };
                            let claim = matter.crush(id, u, part, [x, y, z], &frame);
                            let room = matter.claim_room();
                            assert!(matches!(claim, ClaimResult::Applied),
                                "kind={:?} part={part} cell={x},{y},{z} energy={} room={} unpaid={} deferred={}",
                                u.kind, matter.remaining_mj, room,
                                matches!(claim, ClaimResult::Unaffordable), matches!(claim, ClaimResult::Deferred));
                            removed.push((id, part, [x, y, z]));
                            count[layer] += 1;
                        }
                    }
                }
            }
        }
        let mass = matter
            .debris
            .fragments
            .iter()
            .map(|f| f.mass_g)
            .sum::<u64>();
        let quantity = matter.economy.ledger.in_containers();
        matter.finish_body_batch();
        assert_eq!(
            matter
                .debris
                .fragments
                .iter()
                .map(|f| f.count as usize)
                .sum::<usize>(),
            removed.len()
        );
        assert_eq!(
            matter
                .debris
                .fragments
                .iter()
                .map(|f| f.mass_g)
                .sum::<u64>(),
            mass
        );
        assert_eq!(matter.economy.ledger.in_containers(), quantity);
        assert!(matter
            .debris
            .fragments
            .iter()
            .any(|f| f.chunk.as_ref().is_some_and(|c| c.segment.is_some())));
        for f in &matter.debris.fragments {
            if let Some(chunk) = &f.chunk {
                assert_eq!(
                    f.basis,
                    crate::burst::IDENTITY,
                    "captured pose must not turn twice"
                );
                assert_eq!(f.position, chunk.centre);
                let part = chunk.segment.map_or(0, |s| usize::from(s) + 1);
                let unit = &units[usize::from(chunk.template)];
                assert_eq!(
                    crate::burst::body_fragments::components(
                        chunk.cells.iter().copied().enumerate().map(|(i, c)| (c, i)),
                        256
                    )
                    .len(),
                    1
                );
                for cell in &chunk.cells {
                    let frame = if part == 0 {
                        unit.root_frame(usize::from(cell[1]))
                    } else {
                        unit.seg_frames[part - 1]
                    };
                    let body = if part == 0 {
                        &unit.body
                    } else {
                        &unit.body.segments[part - 1].body
                    };
                    let layer = usize::from(body.turret_y > 0 && cell[1] >= body.turret_y);
                    assert_eq!(
                        chunk.frames[layer], frame,
                        "{part}: aimed frame changed at birth"
                    );
                    assert!(removed.contains(&(
                        usize::from(chunk.template),
                        part,
                        cell.map(usize::from)
                    )));
                }
            }
        }
        for (id, part, [x, y, z]) in removed {
            let b = if part == 0 {
                &mut units[id].body
            } else {
                &mut units[id].body.segments[part - 1].body
            };
            assert!(b.remove(x, y, z));
            // Match the authoritative commit, including Skoba's intrinsic
            // stock (independent of its currently depicted cargo shape).
            let cell = [x, y, z].map(|v| v as u8);
            if part == 0 {
                units[id].removed.push(cell);
                units[id].detached += 1;
            } else {
                units[id].seg_removed.push(((part - 1) as u8, cell));
                units[id].seg_detached += 1;
            }
        }
        struct Air;
        impl voxel::Generator for Air {
            fn material(&self, _: u32, _: u32, _: u32) -> u16 { 0 }
        }
        let world = voxel::World::new(2560, 128, 2560, 4096, Box::new(Air));
        let mut bytes = Blob::new();
        matter.save(&mut bytes);
        let mut copy = Matter::new(&units);
        assert!(copy
            .load(&mut Scan::new(&bytes.v), &world, &units)
            .is_some());
        let mut roundtrip = Blob::new();
        copy.save(&mut roundtrip);
        assert_eq!(bytes.v, roundtrip.v);
        assert!(copy.economy.ledger.balanced());
    }
    #[test]
    fn repeated_hits_after_reaiming_keep_old_panels_and_pay_each_cell_once() {
        let mut unit = RangeUnit::of_kind(modding::range::RangeUnitKind::Tank, [240, 64, 240]);
        let mut matter = Matter::new(std::slice::from_ref(&unit));
        let initial = matter.economy.ledger.held(matter.bodies[0]);
        let mut removed = BTreeSet::new();
        let mut old = Vec::new();
        let mut old_ledger = Vec::new();
        for hit in 0..3 {
            unit.motion.drive.head = fx::angle::FULL / (7 + hit);
            unit.turret_pitch = fx::angle::FULL / (24 + hit);
            unit.update_pose(-fx::angle::FULL / (11 + hit));
            let mut cells = Vec::new();
            let mut count = [0; 2];
            for z in 0..usize::from(unit.body.vol.nz) {
                for y in 0..usize::from(unit.body.vol.ny) {
                    let layer = usize::from(y >= usize::from(unit.body.turret_y));
                    for x in 0..usize::from(unit.body.vol.nx) {
                        if unit.body.vol.get(x, y, z) && count[layer] < 128 {
                            cells.push([x, y, z]);
                            count[layer] += 1;
                        }
                    }
                }
            }
            assert_eq!(count, [128; 2], "both hull and aimed turret must be hit");
            matter.begin(u32::MAX, 4);
            matter.begin_body_batch();
            let start = matter.debris.fragments.len();
            for &cell in &cells {
                assert!(removed.insert(cell), "a removed cell was hit again");
                assert!(matches!(
                    matter.body(0, &unit, cell, &unit.root_frame(cell[1]), [200, 64, 240]),
                    ClaimResult::Applied
                ));
                assert!(
                    matches!(
                        matter.body(0, &unit, cell, &unit.root_frame(cell[1]), [200, 64, 240]),
                        ClaimResult::Deferred
                    ),
                    "duplicate claim must not create matter or geometry"
                );
                assert!(unit.body.remove(cell[0], cell[1], cell[2]));
            }
            let room = matter.finish_body_batch();
            assert_eq!(room, matter.claim_room());
            assert!(matter.debris.fragments[start..]
                .iter()
                .any(|f| f.chunk.is_some()));
            assert_eq!(
                matter
                    .debris
                    .fragments
                    .iter()
                    .map(|f| f.count as usize)
                    .sum::<usize>(),
                removed.len()
            );
            assert_eq!(
                initial - matter.economy.ledger.held(matter.bodies[0]),
                removed.len() as u64 * u64::from(matter.source_density(3))
            );
            assert!(matter.economy.ledger.balanced());
            for (f, (id, position, velocity, basis, chunk_hash)) in
                matter.debris.fragments.iter().zip(&old)
            {
                assert_eq!(
                    (
                        f.id,
                        f.position,
                        f.velocity,
                        f.basis,
                        f.chunk.as_ref().map(|c| c.hash())
                    ),
                    (*id, *position, *velocity, *basis, *chunk_hash),
                    "later aiming/damage changed an earlier panel"
                );
            }
            for &(container, amount) in &old_ledger {
                assert_eq!(matter.economy.ledger.held(container), amount);
            }
            old = matter
                .debris
                .fragments
                .iter()
                .map(|f| {
                    (
                        f.id,
                        f.position,
                        f.velocity,
                        f.basis,
                        f.chunk.as_ref().map(|c| c.hash()),
                    )
                })
                .collect();
            old_ledger = matter
                .debris
                .fragments
                .iter()
                .map(|f| (f.held, matter.economy.ledger.held(f.held)))
                .collect();
        }
    }
    #[test]
    fn armour_batches_keep_matter_and_exact_connected_geometry() {
        let config = modding::explosion::ExplosionConfig::from_ron(include_str!(
            "../../../data/energy-cannon.ron"
        ))
        .unwrap();
        let layout =
            modding::range::RangeLayout::from_ron(include_str!("../../../data/range-cover.ron"))
                .unwrap();
        let mut r = crate::blast::BlastRange::with_layout(config, layout).unwrap();
        let u = &r.units[0];
        let b = &u.body;
        let mut cells = Vec::new();
        for z in 0..usize::from(b.vol.nz) {
            for y in 0..usize::from(b.vol.ny) {
                for x in 0..usize::from(b.vol.nx) {
                    if b.vol.get(x, y, z) {
                        cells.push([x, y, z]);
                    }
                }
            }
        }
        let isolated = *cells.last().unwrap();
        cells.truncate(511);
        cells.push(isolated);
        let m = &mut r.matter;
        m.begin(u32::MAX, 4);
        m.begin_body_batch();
        for &cell in &cells {
            assert!(matches!(
                m.body(0, u, cell, &u.root_frame(cell[1]), [200, 16, 330]),
                ClaimResult::Applied
            ));
        }
        let before = m.debris.fragments.len();
        let mass: u64 = m.debris.fragments.iter().map(|f| f.mass_g).sum();
        let motion: u64 = m.debris.fragments.iter().map(motion_mj).sum();
        let held = m.economy.ledger.in_containers();
        m.finish_body_batch();
        let fragments = &m.debris.fragments;
        assert!(
            fragments.len() < before / 2,
            "{} fragments for {} cells",
            fragments.len(),
            before
        );
        assert!(fragments.iter().any(|f| f.count >= 16));
        assert!(fragments.iter().any(|f| f.count <= 4));
        assert_eq!(
            fragments.iter().map(|f| f.count as usize).sum::<usize>(),
            cells.len()
        );
        assert_eq!(fragments.iter().map(|f| f.mass_g).sum::<u64>(), mass);
        assert!(fragments.iter().map(motion_mj).sum::<u64>() <= motion);
        assert_eq!(m.economy.ledger.in_containers(), held);
        assert!(m.economy.ledger.balanced());
        let mut restored = BTreeSet::new();
        for f in fragments {
            if let Some(chunk) = &f.chunk {
                assert_eq!(
                    crate::burst::body_fragments::components(
                        chunk.cells.iter().copied().enumerate().map(|(i, c)| (c, i)),
                        256
                    )
                    .len(),
                    1
                );
                assert!(chunk.segment.is_none());
                for cell in &chunk.cells {
                    assert!(restored.insert(cell.map(usize::from)));
                }
                continue;
            }
            assert_eq!(shape_components(f.shape, f.side), [f.shape]);
            let centre = shape_centre(f.shape, f.side);
            let scale = Fx::from_ratio(f.mm as i32, voxel::unit::WORLD_MM as i32);
            for bit in 0..u32::from(f.side).pow(3) {
                if f.shape & (1 << bit) == 0 {
                    continue;
                }
                let s = u32::from(f.side);
                let delta = [
                    Fx::from_int((bit % s) as i32).sub(centre.x),
                    Fx::from_int((bit / s % s) as i32).sub(centre.y),
                    Fx::from_int((bit / s / s) as i32).sub(centre.z),
                ];
                let mut world = f.position;
                for (axis, d) in f.basis.iter().zip(delta) {
                    world = world.add(axis.scale(d.mul(scale)));
                }
                // Cells chosen first come from the lower hull in this fixture.
                let local = u
                    .root_frame(0)
                    .local(world)
                    .map(|v| v.to_int_trunc() as usize);
                assert!(restored.insert(local));
            }
        }
        assert_eq!(restored, cells.iter().copied().collect());
        assert!(m.debris.fragments.iter().any(|f| f.chunk.is_some()));
        for [x, y, z] in cells {
            assert!(r.units[0].body.remove(x, y, z));
        }
        let mut bytes = Blob::new();
        m.save(&mut bytes);
        let mut copy = Matter::new(&r.units);
        assert!(copy
            .load(&mut Scan::new(&bytes.v), &r.world, &r.units)
            .is_some());
        let mut restored = Blob::new();
        copy.save(&mut restored);
        assert_eq!(bytes.v, restored.v);
    }
}
