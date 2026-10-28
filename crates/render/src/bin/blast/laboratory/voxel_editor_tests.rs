use super::*;
use modding::laboratory::catalog::{
    self,
    voxels::{self, Tool},
    Category, Element, Placed,
};
use std::collections::BTreeSet;

fn setup() -> Laboratory {
    let mut lab = Laboratory::new(false);
    let c = &mut lab.constructor;
    c.stand = false;
    let e = Element {
        id: "editable-test".into(),
        name: "Проверочная деталь".into(),
        category: Category::Custom,
        blocks: vec![RangeBlock {
            min: [0; 3],
            max: [4; 3],
            material: 2,
        }],
        units: vec![],
        unit_yaws: vec![],
        shields: vec![],
    };
    c.placed = vec![
        Placed {
            element: e.clone(),
            at: [384, 20, 336],
            turns: 1,
        },
        Placed {
            element: e,
            at: [400, 20, 336],
            turns: 0,
        },
    ];
    c.selected = Some(0);
    lab
}
fn action(l: &mut Laboratory, a: LabAction) -> bool {
    l.constructor
        .action(a, &l.definition, &mut l.recipe)
        .unwrap()
        .0
}
fn stroke(l: &mut Laboratory, p: [i32; 3], tool: Tool, mat: u16) {
    l.constructor
        .stage_voxel_cells(BTreeSet::from([p]), tool, mat)
        .unwrap();
    assert!(action(l, LabAction::VoxelStroke));
}
#[test]
fn voxel_editor_scopes_undo_redo_cancel_and_keeps_the_authored_scene_portable() {
    let mut lab = setup();
    let before = lab.constructor.placed.clone();
    let library = lab.constructor.catalog.clone();
    assert!(action(&mut lab, LabAction::BeginVoxelEdit));
    let p = voxels::world_cell(&lab.constructor.placed[0], [1, 1, 1]);
    stroke(&mut lab, p, Tool::Paint, 5);
    let painted = lab.constructor.placed[0].clone();
    assert_eq!(voxels::material(&painted.element, [1, 1, 1]), 5);
    let hole = voxels::world_cell(&painted, [2, 1, 1]);
    stroke(&mut lab, hole, Tool::Erase, 2);
    let erased = lab.constructor.placed[0].clone();
    assert_eq!(voxels::material(&erased.element, [2, 1, 1]), 0);
    assert!(action(&mut lab, LabAction::VoxelUndo));
    assert_eq!(lab.constructor.placed[0], painted);
    assert!(action(&mut lab, LabAction::VoxelRedo));
    assert_eq!(lab.constructor.placed[0], erased);
    assert_eq!(lab.constructor.placed[1], before[1]);
    assert_eq!(lab.constructor.catalog, library);
    let file = catalog::SceneFile {
        version: 1,
        base: lab
            .constructor
            .base_layout(&lab.definition, lab.recipe)
            .unwrap(),
        elements: lab.constructor.placed.clone(),
        weapon: lab.config(),
        strength: None,
        visual: 0,
        experiment: None,
        workspace: lab.constructor.workspace.clone(),
    };
    let loaded = catalog::SceneFile::from_ron(&file.to_ron().unwrap()).unwrap();
    assert_eq!(loaded.elements, file.elements);
    assert!(action(&mut lab, LabAction::VoxelFinish(false)));
    assert_eq!(lab.constructor.placed, before);
    assert!(!lab.constructor.voxel_editing());
    action(&mut lab, LabAction::BeginVoxelEdit);
    stroke(&mut lab, p, Tool::Paint, 3);
    action(&mut lab, LabAction::VoxelFinish(true));
    assert_eq!(
        voxels::material(&lab.constructor.placed[0].element, [1, 1, 1]),
        3
    );
    assert!(!lab.constructor.voxel_editing());
}
#[test]
fn voxel_editor_commits_once_on_release_and_does_not_use_the_world_sculpt_brush() {
    let mut lab = setup();
    action(&mut lab, LabAction::BeginVoxelEdit);
    let p = voxels::world_cell(&lab.constructor.placed[0], [1, 1, 1]);
    lab.tools.edge = 8;
    lab.definition.editor_step = 16;
    lab.constructor
        .stage_voxel_cells(BTreeSet::from([p]), Tool::Paint, 3)
        .unwrap();
    let mut mouse = ButtonInput::default();
    mouse.press(MouseButton::Left);
    constructor::voxels::release(&mut lab, &mouse);
    assert!(lab.constructor.pending.is_empty());
    mouse.release(MouseButton::Left);
    constructor::voxels::release(&mut lab, &mouse);
    constructor::voxels::release(&mut lab, &mouse);
    assert!(lab.constructor.pending == vec![LabAction::VoxelStroke]);
    lab.constructor.pending.clear();
    let revision = lab.constructor.revision;
    action(&mut lab, LabAction::VoxelStroke);
    assert_eq!(lab.constructor.revision, revision + 1);
    assert_eq!(
        voxels::material(&lab.constructor.placed[0].element, [1, 1, 1]),
        3
    );
    assert_eq!(
        voxels::material(&lab.constructor.placed[0].element, [1, 1, 2]),
        2
    );
    assert!(!lab.constructor.voxel_allowed(LabAction::Fire));
    assert!(!lab.constructor.voxel_allowed(LabAction::PickObject(1)));
    assert!(lab.constructor.voxel_allowed(LabAction::PickMaterial(5)));
}
#[test]
fn voxel_editor_screenshot_fixture_contains_actual_cell_changes_and_visible_controls() {
    let mut lab = Laboratory::new(false);
    lab.constructor.stand = false;
    let item = lab
        .constructor
        .catalog
        .iter()
        .find(|e| e.id == "bunker")
        .unwrap()
        .clone();
    lab.constructor.placed = vec![Placed {
        element: item,
        at: [384, 16, 400],
        turns: 0,
    }];
    lab.constructor.selected = Some(0);
    assert!(ui::enabled(&lab, LabAction::BeginVoxelEdit));
    action(&mut lab, LabAction::BeginVoxelEdit);
    lab.ui.tab = 17;
    let b = lab.constructor.placed[0]
        .element
        .blocks
        .iter()
        .find(|b| b.material != 0)
        .unwrap();
    let at = voxels::world_cell(&lab.constructor.placed[0], b.min);
    stroke(&mut lab, at, Tool::Paint, 3);
    stroke(&mut lab, [at[0], at[1] + 1, at[2]], Tool::Erase, 3);
    stroke(&mut lab, [at[0] - 1, at[1], at[2]], Tool::Add, 5);
    assert!(ui::enabled(&lab, LabAction::VoxelFinish(true)));
    assert!(ui::enabled(&lab, LabAction::VoxelTool(2)));
    assert!(lab
        .constructor
        .voxel_text(&lab.definition, lab.recipe)
        .contains("100 × 100 × 100 мм"));
}
