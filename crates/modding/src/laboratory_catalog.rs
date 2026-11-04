//! Portable library elements and authored laboratory scenes. Stable material
//! IDs and embedded geometry survive library reordering and later editing.
use super::{Definition, Recipe};
use crate::range::{RangeBlock, RangeLayout, RangeShieldDef, RangeUnitDef};
use serde::{Deserialize, Serialize};
#[path = "laboratory_voxels.rs"]
pub mod voxels;
pub const MAX_ELEMENT_BLOCKS: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum Category {
    Block,
    Structure,
    Unit,
    Vegetation,
    Shield,
    Custom,
}
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Element {
    pub id: String,
    pub name: String,
    pub category: Category,
    #[serde(default)]
    pub blocks: Vec<RangeBlock>,
    #[serde(default)]
    pub units: Vec<RangeUnitDef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unit_yaws: Vec<u8>,
    #[serde(default)]
    pub shields: Vec<RangeShieldDef>,
}
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Placed {
    pub element: Element,
    pub at: [i32; 3],
    pub turns: u8,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SceneFile {
    pub version: u32,
    pub base: RangeLayout,
    pub elements: Vec<Placed>,
    pub weapon: crate::explosion::ExplosionConfig,
    pub strength: Option<(u16, u32)>,
    #[serde(default)]
    pub visual: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub experiment: Option<super::experiments::Plan>,
    #[serde(default)]
    pub workspace: super::sources::Workspace,
}
#[derive(Deserialize)]
struct Library {
    elements: Vec<Element>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct WeaponAsset {
    pub version: u32,
    pub name: String,
    pub config: crate::explosion::ExplosionConfig,
    pub visual: u8,
}
impl WeaponAsset {
    pub fn validate(&self) -> Result<(), String> {
        if self.version != 1
            || self.name.trim().is_empty()
            || self.name.chars().count() > 64
            || self.visual > 2
        {
            return Err("invalid laboratory weapon".into());
        }
        self.config.validate()
    }
    pub fn from_ron(text: &str) -> Result<Self, String> {
        if text.len() > 65536 {
            return Err("weapon file exceeds limit".into());
        }
        let w: Self = ron::from_str(text).map_err(|e| e.to_string())?;
        w.validate()?;
        Ok(w)
    }
    pub fn to_ron(&self) -> Result<String, String> {
        self.validate()?;
        ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default())
            .map_err(|e| e.to_string())
    }
}

fn rotate(p: [i32; 3], turns: u8) -> [i32; 3] {
    match turns % 4 {
        1 => [-p[2], p[1], p[0]],
        2 => [-p[0], p[1], -p[2]],
        3 => [p[2], p[1], -p[0]],
        _ => p,
    }
}
impl Element {
    pub fn validate(&self) -> Result<(), String> {
        if self.id.is_empty()
            || self.id.len() > 64
            || !self
                .id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            || self.name.trim().is_empty()
            || self.name.chars().count() > 64
            || self.blocks.len() > MAX_ELEMENT_BLOCKS
            || self.units.len() > 4
            || self.unit_yaws.len() > self.units.len()
            || self.unit_yaws.iter().any(|y| *y > 3)
            || self.shields.len() > 4
            || self.blocks.is_empty() && self.units.is_empty() && self.shields.is_empty()
            || self.blocks.iter().any(|b| {
                ![0, 1, 2, 3, 4, 5, 6, 8, 10, 11, 12].contains(&b.material)
                    || (0..3).any(|a| b.min[a] >= b.max[a] || b.min[a] < -128 || b.max[a] > 128)
                    || b.min[1] < 0
            })
            || self.units.iter().any(|u| {
                !u.valid_loadout() || u.at.iter().any(|p| !(-128..=128).contains(p)) || u.at[1] < 0
            })
            || self.shields.iter().any(|s| {
                !crate::shield::SHIELD_RADIUS.contains(&s.radius)
                    || s.at.iter().any(|p| !(-128..=128).contains(p))
                    || s.at[1] < 0
            })
            || self
                .blocks
                .iter()
                .filter(|b| b.material != 0)
                .map(|b| {
                    (0..3)
                        .map(|a| (b.max[a] - b.min[a]) as u64)
                        .product::<u64>()
                })
                .sum::<u64>()
                > 131072
        {
            return Err("invalid or oversized laboratory element".into());
        }
        Ok(())
    }
    pub fn from_ron(text: &str) -> Result<Self, String> {
        if text.len() > 1 << 20 {
            return Err("element file exceeds limit".into());
        }
        let e: Self = ron::from_str(text).map_err(|e| e.to_string())?;
        e.validate()?;
        Ok(e)
    }
    pub fn to_ron(&self) -> Result<String, String> {
        self.validate()?;
        ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default())
            .map_err(|e| e.to_string())
    }
}
impl Placed {
    pub fn append(&self, layout: &mut RangeLayout) -> Result<(), String> {
        self.element.validate()?;
        if self.turns > 3 || self.at.iter().any(|p| !(-16384..=16384).contains(p)) {
            return Err("Недопустимое положение элемента".into());
        }
        let translate = |p: [i32; 3]| {
            let r = rotate(p, self.turns);
            std::array::from_fn(|a| r[a] + self.at[a])
        };
        for b in &self.element.blocks {
            let mut min = [i32::MAX; 3];
            let mut max = [i32::MIN; 3];
            for x in [b.min[0], b.max[0]] {
                for y in [b.min[1], b.max[1]] {
                    for z in [b.min[2], b.max[2]] {
                        let p = translate([x, y, z]);
                        for a in 0..3 {
                            min[a] = min[a].min(p[a]);
                            max[a] = max[a].max(p[a]);
                        }
                    }
                }
            }
            layout.blocks.push(RangeBlock {
                min,
                max,
                material: b.material,
            });
        }
        if !self.element.units.is_empty()
            && (self.turns != 0
                || !self.element.unit_yaws.is_empty()
                || !layout.unit_yaws.is_empty())
        {
            layout.unit_yaws = (0..layout.units.len())
                .map(|i| layout.unit_yaw(i))
                .collect();
        }
        for (i, u) in self.element.units.iter().enumerate() {
            let mut u = u.clone();
            u.at = translate(u.at);
            if self.turns != 0 || !self.element.unit_yaws.is_empty() || !layout.unit_yaws.is_empty()
            {
                let yaw = self
                    .element
                    .unit_yaws
                    .get(i)
                    .copied()
                    .unwrap_or(if layout.battle && !u.allied { 2 } else { 0 });
                layout.unit_yaws.push((yaw + self.turns) % 4);
            }
            layout.units.push(u);
        }
        for s in &self.element.shields {
            let mut s = s.clone();
            s.at = translate(s.at);
            layout.shields.push(s);
        }
        Ok(())
    }
}
pub fn stock() -> Vec<Element> {
    let mut l: Library =
        ron::from_str(include_str!("../../../data/lab-elements.ron")).expect("lab elements RON");
    let structures: Library = ron::from_str(include_str!("../../../data/lab-structures.ron"))
        .expect("lab structures RON");
    l.elements.extend(structures.elements);
    for (i, e) in l.elements.iter().enumerate() {
        e.validate().expect("valid stock element");
        assert!(!l.elements[..i].iter().any(|other| other.id == e.id));
    }
    l.elements
}
pub fn compose(base: &RangeLayout, items: &[Placed]) -> Result<RangeLayout, String> {
    base.validate()?;
    if items.len() > 32 {
        return Err("Лимит сцены: 32 элемента".into());
    }
    let mut scene = base.clone();
    for item in items {
        item.append(&mut scene)?;
    }
    // Canonical omission keeps old default layouts and snapshots byte-identical.
    while let Some(yaw) = scene.unit_yaws.last().copied() {
        let i = scene.unit_yaws.len() - 1;
        let default = if scene.battle && !scene.units[i].allied {
            2
        } else {
            0
        };
        if yaw != default {
            break;
        }
        scene.unit_yaws.pop();
    }
    scene.validate()?;
    let volume: u64 = scene
        .blocks
        .iter()
        .filter(|b| b.material != 0)
        .map(|b| {
            (0..3)
                .map(|a| (b.max[a] - b.min[a]) as u64)
                .product::<u64>()
        })
        .sum();
    if volume > 524288 {
        return Err("Превышен объём испытательной сцены".into());
    }
    Ok(scene)
}
impl SceneFile {
    pub fn from_ron(text: &str) -> Result<Self, String> {
        if text.len() > 2 << 20 {
            return Err("laboratory scene exceeds file limit".into());
        }
        let s: Self = ron::from_str(text).map_err(|e| e.to_string())?;
        s.validate()?;
        Ok(s)
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.version != 1
            || self.visual > 2
            || self.base.size[0] > 4096
            || self.base.size[2] > 4096
            || self.base.size[1] > 1024
        {
            return Err("unsupported laboratory scene version or size".into());
        }
        self.weapon.validate()?;
        self.workspace.validate(&self.base, self.elements.len())?;
        if self.workspace.sources.iter().any(|s| {
            s.binding
                .is_some_and(|i| self.elements[i].element.units.is_empty())
        }) {
            return Err("Источник должен быть привязан к элементу с юнитом".into());
        }
        if self.strength.is_some_and(|(id, n)| {
            ![1, 2, 3, 4, 5, 6, 8, 10, 11, 12].contains(&id) || n == 0 || n > 1000000
        }) {
            return Err("invalid laboratory material override".into());
        }
        compose(&self.base, &self.elements)?;
        if let Some(plan) = &self.experiment {
            plan.validate_sources(
                &compose(&self.base, &self.elements)?,
                &self.workspace.sources,
            )?;
        } else if self.workspace.sources.iter().any(|s| s.input.is_some()) {
            return Err("Источник ссылается на отсутствующий сценарий".into());
        }
        Ok(())
    }
    pub fn to_ron(&self) -> Result<String, String> {
        self.validate()?;
        ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default())
            .map_err(|e| e.to_string())
    }
}
pub fn form_element(
    d: &Definition,
    r: Recipe,
    id: String,
    name: String,
) -> Result<Element, String> {
    let (layout, _) = d.fixture(Recipe { effect: 0, ..r })?;
    let origin = [
        d.layout.target[0],
        d.layout.ground_level,
        d.layout.target[2],
    ];
    let e = Element {
        id,
        name,
        category: Category::Custom,
        blocks: layout
            .blocks
            .into_iter()
            .map(|mut b| {
                for a in 0..3 {
                    b.min[a] -= origin[a];
                    b.max[a] -= origin[a];
                }
                b
            })
            .collect(),
        units: Vec::new(),
        unit_yaws: Vec::new(),
        shields: Vec::new(),
    };
    e.validate()?;
    Ok(e)
}
pub fn combine(
    items: &[Placed],
    origin: [i32; 3],
    id: String,
    name: String,
    base: &RangeLayout,
) -> Result<Element, String> {
    if items.is_empty() {
        return Err("Добавьте хотя бы одну деталь".into());
    }
    let mut empty = base.clone();
    empty.blocks.clear();
    empty.units.clear();
    empty.unit_yaws.clear();
    empty.shields.clear();
    let layout = compose(&empty, items)?;
    let unit_yaws = (0..layout.units.len())
        .map(|i| layout.unit_yaw(i))
        .collect();
    let e = Element {
        id,
        name,
        category: Category::Custom,
        blocks: layout
            .blocks
            .into_iter()
            .map(|mut b| {
                for a in 0..3 {
                    b.min[a] -= origin[a];
                    b.max[a] -= origin[a];
                }
                b
            })
            .collect(),
        unit_yaws,
        units: layout
            .units
            .into_iter()
            .map(|mut u| {
                for a in 0..3 {
                    u.at[a] -= origin[a];
                }
                u
            })
            .collect(),
        shields: layout
            .shields
            .into_iter()
            .map(|mut s| {
                for a in 0..3 {
                    s.at[a] -= origin[a];
                }
                s
            })
            .collect(),
    };
    e.validate()?;
    Ok(e)
}
/// Flatten one prefab for editing without changing its resolved geometry or order.
pub fn parts(item: &Placed, base: &RangeLayout) -> Result<Vec<Placed>, String> {
    let mut layout = base.clone();
    layout.blocks.clear();
    layout.units.clear();
    layout.unit_yaws.clear();
    layout.shields.clear();
    item.append(&mut layout)?;
    let mut result = parts_of_layout(layout)?;
    for p in &mut result {
        if !p.element.units.is_empty() {
            p.element.name = item.element.name.clone();
        }
    }
    Ok(result)
}
/// The constructor and authored experiments share one geometry decomposition.
pub fn parts_of_layout(layout: RangeLayout) -> Result<Vec<Placed>, String> {
    let mut result = Vec::new();
    let yaws: Vec<_> = (0..layout.units.len())
        .map(|i| layout.unit_yaw(i))
        .collect();
    for mut b in layout.blocks {
        let at = [
            (b.min[0] + b.max[0]) / 2,
            b.min[1],
            (b.min[2] + b.max[2]) / 2,
        ];
        for a in 0..3 {
            b.max[a] -= at[a];
            b.min[a] -= at[a];
        }
        result.push(Placed {
            element: Element {
                id: "editable-block".into(),
                name: "Деталь сборки".into(),
                category: Category::Custom,
                blocks: vec![b],
                units: Vec::new(),
                unit_yaws: Vec::new(),
                shields: Vec::new(),
            },
            at,
            turns: 0,
        });
    }
    for (i, mut u) in layout.units.into_iter().enumerate() {
        let at = u.at;
        u.at = [0; 3];
        result.push(Placed {
            element: Element {
                id: "editable-unit".into(),
                name: "Юнит".into(),
                category: Category::Unit,
                blocks: Vec::new(),
                units: vec![u],
                unit_yaws: vec![yaws[i]],
                shields: Vec::new(),
            },
            at,
            turns: 0,
        });
    }
    for mut s in layout.shields {
        let at = s.at;
        s.at = [0; 3];
        result.push(Placed {
            element: Element {
                id: "editable-shield".into(),
                name: "Щит сборки".into(),
                category: Category::Shield,
                blocks: Vec::new(),
                units: Vec::new(),
                unit_yaws: Vec::new(),
                shields: vec![s],
            },
            at,
            turns: 0,
        });
    }
    for p in &result {
        p.element.validate()?;
    }
    Ok(result)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn laboratory_unit_yaws_preserve_legacy_defaults_and_reject_invalid_metadata() {
        let d = Definition::stock();
        assert!(!d.layout.to_ron().unwrap().contains("unit_yaws"));
        let tank = stock().into_iter().find(|e| e.id == "tank").unwrap();
        let mut placed = Placed {
            element: tank,
            at: [384, 16, 336],
            turns: 0,
        };
        let default = compose(&d.layout, std::slice::from_ref(&placed)).unwrap();
        assert!(default.unit_yaws.is_empty());
        placed.turns = 3;
        let rotated = compose(&d.layout, std::slice::from_ref(&placed)).unwrap();
        assert_eq!(rotated.unit_yaw(0), 3);
        assert_eq!(
            RangeLayout::from_ron(&rotated.to_ron().unwrap()).unwrap(),
            rotated
        );
        let mut bad = d.layout.clone();
        bad.unit_yaws = vec![4];
        assert!(compose(&bad, &[]).is_err());
        let mut bad_element = placed.element.clone();
        bad_element.unit_yaws = vec![4];
        assert!(bad_element.validate().is_err());
    }
    #[test]
    fn library_composes_rotates_and_roundtrips_without_referencing_mutable_sources() {
        let d = Definition::stock();
        let library = stock();
        for element in &library {
            let item = Placed {
                element: element.clone(),
                at: [384, 16, 336],
                turns: 0,
            };
            compose(&d.layout, std::slice::from_ref(&item)).unwrap();
            let text = element.to_ron().unwrap();
            assert_eq!(Element::from_ron(&text).unwrap(), *element);
        }
        let mut item = Placed {
            element: library[0].clone(),
            at: [384, 16, 336],
            turns: 0,
        };
        let a = compose(&d.layout, std::slice::from_ref(&item)).unwrap();
        item.turns = 1;
        let b = compose(&d.layout, std::slice::from_ref(&item)).unwrap();
        assert_eq!(
            a.blocks[0].max[0] - a.blocks[0].min[0],
            b.blocks[0].max[2] - b.blocks[0].min[2]
        );
        item.at = [i32::MAX, 0, 0];
        assert!(compose(&d.layout, &[item]).is_err());
    }
    #[test]
    fn disassembly_preserves_rotated_geometry_and_independent_materials() {
        let d = Definition::stock();
        let item = Placed {
            element: stock().into_iter().find(|e| e.id == "bridge").unwrap(),
            at: [384, 16, 336],
            turns: 1,
        };
        let before = compose(&d.layout, std::slice::from_ref(&item)).unwrap();
        let mut parts = parts(&item, &d.layout).unwrap();
        let after = compose(&d.layout, &parts).unwrap();
        assert_eq!(before.blocks, after.blocks);
        assert_eq!(parts.len(), 3);
        parts[2].element.blocks[0].material = 5;
        let edited = compose(&d.layout, &parts).unwrap();
        assert_eq!(edited.blocks[0].material, 2);
        assert_eq!(edited.blocks[2].material, 5);
        assert_eq!(before.blocks[2].material, 3);
    }
}
