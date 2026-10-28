//! Exact minimum-cell authoring using compact, nonoverlapping cuboids.
//! No volume scan; empty cuts never escape the edited element into neighbours.
use super::*;
use std::collections::BTreeSet;
const WORK_BLOCKS: usize = 4096;
pub const STROKE_CELLS: usize = 256;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Add,
    Erase,
    Paint,
}
pub fn local_cell(item: &Placed, p: [i32; 3]) -> [i32; 3] {
    let [x, y, z] = std::array::from_fn(|a| p[a] - item.at[a]);
    match item.turns % 4 {
        1 => [z, y, -x - 1],
        2 => [-x - 1, y, -z - 1],
        3 => [-z - 1, y, x],
        _ => [x, y, z],
    }
}
pub fn world_cell(item: &Placed, p: [i32; 3]) -> [i32; 3] {
    let [x, y, z] = p;
    let q = match item.turns % 4 {
        1 => [-z - 1, y, x],
        2 => [-x - 1, y, -z - 1],
        3 => [z, y, -x - 1],
        _ => p,
    };
    std::array::from_fn(|a| q[a] + item.at[a])
}
pub fn material(e: &Element, p: [i32; 3]) -> u16 {
    e.blocks
        .iter()
        .rev()
        .find(|b| b.contains(p))
        .map_or(0, |b| b.material)
}
fn subtract(b: &RangeBlock, cut: &RangeBlock, out: &mut Vec<RangeBlock>) {
    let lo: [i32; 3] = std::array::from_fn(|a| b.min[a].max(cut.min[a]));
    let hi: [i32; 3] = std::array::from_fn(|a| b.max[a].min(cut.max[a]));
    if (0..3).any(|a| lo[a] >= hi[a]) {
        out.push(b.clone());
        return;
    }
    let mut middle = b.clone();
    for a in 0..3 {
        if middle.min[a] < lo[a] {
            let mut piece = middle.clone();
            piece.max[a] = lo[a];
            out.push(piece);
            middle.min[a] = lo[a];
        }
        if middle.max[a] > hi[a] {
            let mut piece = middle.clone();
            piece.min[a] = hi[a];
            out.push(piece);
            middle.max[a] = hi[a];
        }
    }
}
fn overlay(blocks: &[RangeBlock], b: &RangeBlock) -> Result<Vec<RangeBlock>, String> {
    let mut next = Vec::new();
    for old in blocks {
        subtract(old, b, &mut next);
        if next.len() > WORK_BLOCKS {
            return Err("Слишком сложная деталь: уменьшите число разрозненных кубиков".into());
        }
    }
    if b.material != 0 {
        next.push(b.clone());
    }
    Ok(next)
}
fn compact(blocks: &mut Vec<RangeBlock>) {
    // Three stable sorting/merging passes, never an all-pairs merge loop.
    for a in 0..3 {
        let b = (a + 1) % 3;
        let c = (a + 2) % 3;
        blocks.sort_by_key(|v| {
            (
                v.material, v.min[b], v.max[b], v.min[c], v.max[c], v.min[a], v.max[a],
            )
        });
        let mut merged: Vec<RangeBlock> = Vec::new();
        for v in blocks.drain(..) {
            if let Some(last) = merged.last_mut() {
                if last.material == v.material
                    && last.min[b] == v.min[b]
                    && last.max[b] == v.max[b]
                    && last.min[c] == v.min[c]
                    && last.max[c] == v.max[c]
                    && last.max[a] == v.min[a]
                {
                    last.max[a] = v.max[a];
                    continue;
                }
            }
            merged.push(v);
        }
        *blocks = merged;
    }
}
pub fn edit(
    e: &Element,
    cells: &BTreeSet<[i32; 3]>,
    tool: Tool,
    mat: u16,
) -> Result<Element, String> {
    e.validate()?;
    if !e.units.is_empty() || !e.shields.is_empty() {
        return Err(
            "Выберите блочную деталь. Геометрия юнита и поле щита редактируются отдельно".into(),
        );
    }
    if cells.len() > STROKE_CELLS
        || ![1, 2, 3, 4, 5, 6, 8, 10, 11, 12].contains(&mat)
        || cells.iter().any(|p| {
            p[0] < -128 || p[0] >= 128 || p[1] < 0 || p[1] >= 128 || p[2] < -128 || p[2] >= 128
        })
    {
        return Err("Правка выходит за границы детали или лимит мазка".into());
    }
    let mut blocks = Vec::new();
    for b in &e.blocks {
        blocks = overlay(&blocks, b)?;
    }
    let mut changed = false;
    for &p in cells {
        let old = blocks
            .iter()
            .find(|b| b.contains(p))
            .map_or(0, |b| b.material);
        let value = match tool {
            Tool::Add if old == 0 => mat,
            Tool::Erase if old != 0 => 0,
            Tool::Paint if old != 0 => mat,
            _ => old,
        };
        if old == value {
            continue;
        }
        changed = true;
        blocks = overlay(
            &blocks,
            &RangeBlock {
                min: p,
                max: p.map(|v| v + 1),
                material: value,
            },
        )?;
    }
    if !changed {
        return Ok(e.clone());
    }
    compact(&mut blocks);
    if blocks.len() > MAX_ELEMENT_BLOCKS {
        return Err(format!("Слишком сложная деталь: предел {MAX_ELEMENT_BLOCKS} компактных участков. Мазок не применён"));
    }
    if blocks.is_empty() {
        return Err(
            "Последний кубик детали удалять нельзя; удалите весь объект кнопкой «Удалить»".into(),
        );
    }
    let mut next = e.clone();
    next.blocks = blocks;
    next.validate()?;
    Ok(next)
}
pub fn edit_placed(
    item: &Placed,
    world: &BTreeSet<[i32; 3]>,
    tool: Tool,
    mat: u16,
) -> Result<Placed, String> {
    if world.len() > STROKE_CELLS {
        return Err("Слишком длинный мазок".into());
    }
    let mut cells: BTreeSet<_> = world.iter().map(|&p| local_cell(item, p)).collect();
    let lower = cells.iter().map(|p| p[1]).min().unwrap_or(0).min(0);
    let mut next = item.clone();
    if lower < 0 {
        if lower < -128 {
            return Err("Кубик слишком далеко от детали".into());
        }
        for b in &mut next.element.blocks {
            b.min[1] -= lower;
            b.max[1] -= lower;
        }
        cells = cells
            .into_iter()
            .map(|mut p| {
                p[1] -= lower;
                p
            })
            .collect();
        next.at[1] += lower;
    }
    next.element = edit(&next.element, &cells, tool, mat)?;
    Ok(next)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn cube() -> Element {
        Element {
            id: "voxel-test".into(),
            name: "Куб".into(),
            category: Category::Custom,
            blocks: vec![RangeBlock {
                min: [0; 3],
                max: [4; 3],
                material: 2,
            }],
            units: vec![],
            unit_yaws: vec![],
            shields: vec![],
        }
    }
    fn volume(e: &Element) -> u64 {
        e.blocks
            .iter()
            .map(|b| {
                (0..3)
                    .map(|a| (b.max[a] - b.min[a]) as u64)
                    .product::<u64>()
            })
            .sum()
    }
    #[test]
    fn voxel_edit_changes_one_cell_preserves_holes_layers_and_never_emits_air_cuts() {
        let mut e = cube();
        e.blocks.push(RangeBlock {
            min: [1, 0, 1],
            max: [3, 4, 3],
            material: 0,
        });
        e.blocks.push(RangeBlock {
            min: [0, 3, 0],
            max: [4, 4, 4],
            material: 5,
        });
        let original = e.clone();
        for (p, tool, mat) in [
            ([0, 1, 0], Tool::Erase, 2),
            ([3, 1, 3], Tool::Paint, 3),
            ([4, 1, 3], Tool::Add, 10),
        ] {
            e = edit(&e, &BTreeSet::from([p]), tool, mat).unwrap();
        }
        for x in -1..6 {
            for y in 0..5 {
                for z in -1..6 {
                    let p = [x, y, z];
                    let expected = match p {
                        [0, 1, 0] => 0,
                        [3, 1, 3] => 3,
                        [4, 1, 3] => 10,
                        _ => material(&original, p),
                    };
                    assert_eq!(material(&e, p), expected, "{p:?}");
                }
            }
        }
        assert!(e.blocks.iter().all(|b| b.material != 0));
        assert_eq!(Element::from_ron(&e.to_ron().unwrap()).unwrap(), e);
    }
    #[test]
    fn voxel_edit_rotated_world_cells_roundtrip_without_mirroring_off_by_one() {
        let definition = Definition::stock();
        for turns in 0..4 {
            let item = Placed {
                element: cube(),
                at: [384, 16, 336],
                turns,
            };
            for local in [[0, 0, 0], [3, 2, 1], [-3, 1, 4]] {
                assert_eq!(local_cell(&item, world_cell(&item, local)), local);
            }
            let p = world_cell(&item, [1, 1, 1]);
            let changed = edit_placed(&item, &BTreeSet::from([p]), Tool::Paint, 3).unwrap();
            let mut layout = definition.layout.clone();
            layout.blocks.clear();
            changed.append(&mut layout).unwrap();
            assert_eq!(
                layout
                    .blocks
                    .iter()
                    .rev()
                    .find(|b| b.contains(p))
                    .unwrap()
                    .material,
                3
            );
            assert_eq!(volume(&changed.element), 64);
        }
    }
    #[test]
    fn voxel_edit_can_extend_under_a_part_without_moving_its_existing_cells() {
        let item = Placed {
            element: cube(),
            at: [384, 20, 336],
            turns: 1,
        };
        let p = world_cell(&item, [1, -1, 1]);
        let changed = edit_placed(&item, &BTreeSet::from([p]), Tool::Add, 5).unwrap();
        assert_eq!(changed.at[1], 19);
        assert_eq!(material(&changed.element, local_cell(&changed, p)), 5);
        for x in 0..4 {
            for y in 0..4 {
                for z in 0..4 {
                    let world = world_cell(&item, [x, y, z]);
                    assert_eq!(material(&changed.element, local_cell(&changed, world)), 2);
                }
            }
        }
        assert_eq!(volume(&changed.element), 65);
    }
    #[test]
    fn voxel_edit_compacts_a_full_stroke_and_rejects_excessive_complexity_atomically() {
        let mut seed = cube();
        seed.blocks[0].max = [1; 3];
        let sheet: BTreeSet<_> = (0..16)
            .flat_map(|x| (0..16).map(move |z| [x, 1, z]))
            .collect();
        let result = edit(&seed, &sheet, Tool::Add, 2).unwrap();
        assert_eq!(volume(&result), 257);
        assert!(result.blocks.len() <= 2);
        let isolated: BTreeSet<_> = (0..16)
            .flat_map(|x| (0..16).map(move |z| [x * 2, 1, z * 2]))
            .collect();
        assert!(edit(&seed, &isolated, Tool::Add, 5).is_err());
        assert_eq!(seed.blocks.len(), 1);
        assert_eq!(volume(&seed), 1);
        assert!(edit(&seed, &BTreeSet::from([[0, 0, 0]]), Tool::Erase, 2).is_err());
        assert!(edit(&seed, &BTreeSet::from([[128, 0, 0]]), Tool::Add, 2).is_err());
    }
}
