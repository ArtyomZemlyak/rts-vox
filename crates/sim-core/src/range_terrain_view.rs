//! Bounded, owned raw terrain inputs. GPU packing and surface colours belong
//! to the consumer; no world reference survives a publication.
use super::BlastRange;
use std::ops::Range;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Header {
    pub generation: u64,
    pub tick: u32,
    pub size: [u32; 3],
    /// Costs affecting surface work, independent of damage revision/tick.
    pub costs: [u32; 3],
}
impl Header {
    pub fn of(world: &BlastRange, generation: u64) -> Self {
        Self {
            generation,
            tick: world.tick,
            size: world.size(),
            costs: [
                world.config.soil_resistance,
                world.config.concrete_resistance,
                world.config.steel_resistance,
            ],
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub x: Range<i32>,
    pub y: Range<i32>,
    pub z: i32,
}

/// Consumer admission limits; not simulation budgets. A request is charged
/// for its full rectangle, even when most cells would be empty sky.
#[derive(Clone, Copy, Debug)]
pub struct Budget {
    pub columns: usize,
    pub cells: usize,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidRequest,
    BudgetExceeded,
}

fn valid_span(span: &Range<i32>, size: u32) -> bool {
    span.start >= 0 && span.start < span.end && span.end as u32 <= size
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Column {
    /// Unclipped exclusive world ceiling, including solids above the request.
    pub top: u32,
    pub offset: usize,
    pub len: usize,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Damage {
    pub position: [i32; 3],
    pub material: u16,
    pub work: u32,
    pub cost: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Row {
    pub header: Header,
    pub request: Option<Request>,
    pub columns: Vec<Column>,
    /// Consecutive raw columns, cropped at their ceiling. No packed colours.
    pub materials: Vec<u16>,
    pub damage: Vec<Damage>,
}
impl Row {
    /// Fresh capture even on pause. Invalid/budget-exceeding requests leave
    /// the previous publication untouched; accepted captures reuse vectors.
    pub fn update(
        &mut self,
        world: &BlastRange,
        generation: u64,
        request: Request,
        budget: Budget,
    ) -> Result<(), Error> {
        self.capture(world, generation, request, budget, false)
    }
    /// Explicit air padding for nonnegative rectangles extending beyond the
    /// world. The full padded rectangle still consumes the admission budget.
    pub fn update_padded(
        &mut self,
        world: &BlastRange,
        generation: u64,
        request: Request,
        budget: Budget,
    ) -> Result<(), Error> {
        self.capture(world, generation, request, budget, true)
    }
    fn capture(
        &mut self,
        world: &BlastRange,
        generation: u64,
        request: Request,
        budget: Budget,
        padded: bool,
    ) -> Result<(), Error> {
        let header = Header::of(world, generation);
        if !valid_span(
            &request.x,
            if padded {
                i32::MAX as u32
            } else {
                header.size[0]
            },
        ) || !valid_span(
            &request.y,
            if padded {
                i32::MAX as u32
            } else {
                header.size[1]
            },
        ) || request.z < 0
            || (!padded && request.z as u32 >= header.size[2])
        {
            return Err(Error::InvalidRequest);
        }
        let width = (request.x.end - request.x.start) as usize;
        let height = (request.y.end - request.y.start) as usize;
        if width > budget.columns
            || width
                .checked_mul(height)
                .is_none_or(|cells| cells > budget.cells)
        {
            return Err(Error::BudgetExceeded);
        }
        self.header = header;
        self.columns.clear();
        self.materials.clear();
        self.damage.clear();
        for x in request.x.clone() {
            let top = world.world.column_ceiling(x as u32, request.z as u32);
            let len = (top
                .min(request.y.end as u32)
                .saturating_sub(request.y.start as u32)) as usize;
            let offset = self.materials.len();
            self.materials.resize(offset + len, 0);
            world.world.column(
                x as u32,
                request.z as u32,
                request.y.start as u32,
                &mut self.materials[offset..],
            );
            self.columns.push(Column { top, offset, len });
        }
        // Surface work is X-ordered and sparse: do not query it for every
        // intact voxel. Only work inside the bounded rectangle is copied.
        for (&position, &work) in world
            .surface_work
            .range([request.x.start, i32::MIN, i32::MIN]..[request.x.end, i32::MIN, i32::MIN])
        {
            if position[2] != request.z || !request.y.contains(&position[1]) {
                continue;
            }
            let column = self.columns[(position[0] - request.x.start) as usize];
            let y = (position[1] - request.y.start) as usize;
            let material = if y < column.len {
                self.materials[column.offset + y]
            } else {
                0
            };
            self.damage.push(Damage {
                position,
                material,
                work,
                cost: super::super::material_work(material, &world.config).max(1),
            });
        }
        self.request = Some(request);
        Ok(())
    }
    pub fn column(&self, x: i32) -> Option<(&Column, &[u16])> {
        let request = self.request.as_ref()?;
        if !request.x.contains(&x) {
            return None;
        }
        let column = self.columns.get((x - request.x.start) as usize)?;
        Some((
            column,
            &self.materials[column.offset..column.offset + column.len],
        ))
    }
    /// Logical used payload, not retained capacity or allocator/RSS usage.
    pub fn payload_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.columns.len() * std::mem::size_of::<Column>()
            + self.materials.len() * std::mem::size_of::<u16>()
            + self.damage.len() * std::mem::size_of::<Damage>()
    }
}

/// Changed rows of a requested X/Z rectangle, not a durable event journal.
/// Every completed tick must be observed; repeated updates are harmless.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Changes {
    pub header: Header,
    pub rows: Vec<i32>,
    touched: Vec<bool>,
}
impl Changes {
    pub fn update(
        &mut self,
        world: &BlastRange,
        generation: u64,
        x: Range<i32>,
        z: Range<i32>,
        max_rows: usize,
    ) -> Result<(), Error> {
        self.capture(world, generation, x, z, max_rows, false)
    }
    /// The same explicit air-padding policy as Row. Only real source changes
    /// are observed; padded rows are still charged against max_rows.
    pub fn update_padded(
        &mut self,
        world: &BlastRange,
        generation: u64,
        x: Range<i32>,
        z: Range<i32>,
        max_rows: usize,
    ) -> Result<(), Error> {
        self.capture(world, generation, x, z, max_rows, true)
    }
    fn capture(
        &mut self,
        world: &BlastRange,
        generation: u64,
        x: Range<i32>,
        z: Range<i32>,
        max_rows: usize,
        padded: bool,
    ) -> Result<(), Error> {
        let header = Header::of(world, generation);
        if !valid_span(
            &x,
            if padded {
                i32::MAX as u32
            } else {
                header.size[0]
            },
        ) || !valid_span(
            &z,
            if padded {
                i32::MAX as u32
            } else {
                header.size[2]
            },
        ) {
            return Err(Error::InvalidRequest);
        }
        let count = (z.end - z.start) as usize;
        if count > max_rows {
            return Err(Error::BudgetExceeded);
        }
        self.header = header;
        self.rows.clear();
        self.touched.resize(count, false);
        self.touched.fill(false);
        let mut touch = |px: i32, pz: i32| {
            if x.contains(&px) && z.contains(&pz) {
                self.touched[(pz - z.start) as usize] = true;
            }
        };
        for edit in world.world.edits() {
            touch(edit.x as i32, edit.z as i32);
        }
        for position in &world.surface_changed {
            touch(position[0], position[2]);
        }
        self.rows.extend(
            self.touched
                .iter()
                .enumerate()
                .filter_map(|(i, &changed)| changed.then_some(z.start + i as i32)),
        );
        Ok(())
    }
    pub fn payload_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.rows.len() * std::mem::size_of::<i32>()
            + self.touched.len()
    }
}

#[cfg(test)]
#[path = "range_terrain_view_tests.rs"]
mod tests;
