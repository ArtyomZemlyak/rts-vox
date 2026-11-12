//! Local route repair after avoidance. Reads live geometry, no search or cache.
use super::*;

impl Motion {
    pub(in crate::blast) fn wants_visible_shortcut(&self) -> bool {
        self.flat_detours && self.enabled && self.pilot.is_none()
            && self.destination.is_some() && !self.searching()
            && !self.blocked && !self.needs_recovery && self.escape_left == 0
            && self.cursor < self.path.len()
            && (self.partial_path || self.cursor + 1 < self.path.len())
    }

    /// One bounded candidate: the current order, never a later queued order.
    /// Failure retains the usable route. The phase admits only a shared quota
    /// of these proofs, independently of its search-start quota.
    pub(in crate::blast) fn shorten_visible_route(&mut self, world: &World, body: &UnitBody,
        others: &[Obstacle], rubble: &nav::vehicle::Rubble) -> bool {
        if !self.wants_visible_shortcut() { return false; }
        let goal = self.destination.unwrap();
        let from = [self.pos[0], self.pos[2]];
        let end = goal.map(Fx::from_int);
        let shape = self.shape(body);
        // Same local scale as nearby hull planning. A cross-map command never
        // turns this optional repair into an unmetered global visibility scan.
        let range = self.navigation.global_min_distance.max(
            4 * (shape.half[0].max(shape.half[1]) + self.navigation.cell_units));
        if (0..2).any(|a| end[a].sub(from[a]).abs() > Fx::from_int(range)) { return false; }
        let way = angle::atan2(end[1].sub(from[1]).raw(), end[0].sub(from[0]).raw());
        if !self.leg_side_guard
            && angle::cos(angle::diff(way, self.drive.head))
                < angle::cos(self.navigation.recovery_turn_deg * angle::FULL / 360) {
            return false;
        }
        let own = Obstacle::of(self, body);
        let radius = own.half[0].mul(own.half[0]).add(own.half[1].mul(own.half[1])).sqrt()
            .add(Fx::from_int(self.navigation.plan_margin));
        // Includes moving hulls too, at their serially accepted position.
        // A swept circumscribed circle covers every intermediate hull yaw.
        if others.iter().any(|o| o.overlaps_height(own.pos[1], own.height)
            && steering::near_preview_box(o, [angle::cos(o.head), angle::sin(o.head)],
                from, end, radius)) { return false; }
        let x = Fx::from_int(shape.half[0].max(shape.support[0]));
        let z = Fx::from_int(shape.half[1].max(shape.support[1]));
        let reach = (x.mul(x).add(z.mul(z)).sqrt().to_int_trunc() + 1
            + shape.span.max(0) + shape.step.max(0) + 2)
            .max(radius.to_int_trunc() + 1);
        let lo = [0, 1].map(|a| from[a].min(end[a]).to_int_trunc() - reach - 1);
        let hi = [0, 1].map(|a| from[a].max(end[a]).to_int_trunc() + reach + 1);
        if lo.iter().any(|&v| v < 0) || hi[0] >= world.nx as i32
            || hi[1] >= world.nz as i32 || rubble.touches(lo, hi) { return false; }
        // A positive shared capability certificate proves a bare level area.
        // Unknown/mixed/changed tiles retain the old path; no exact voxel
        // scans or cache warming are started by this optional optimization.
        if world.prepared_floor_passage(lo.map(|v| v as u32 / 8),
            hi.map(|v| v as u32 / 8 + 1), self.navigation.fell.materials, false, 0)
            != Some(true) { return false; }
        self.path.clear();
        self.path.push(goal);
        self.cursor = 0;
        self.partial_path = false;
        true
    }
}
