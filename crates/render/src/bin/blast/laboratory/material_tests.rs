use super::*;
use modding::laboratory::catalog::{self, SceneFile};
fn assembly() -> catalog::Element {
    catalog::Element {
        id: "material-test".into(),
        name: "Проверочная сборка".into(),
        category: catalog::Category::Custom,
        blocks: vec![
            RangeBlock {
                min: [-4, 0, -4],
                max: [4, 8, 4],
                material: 2,
            },
            RangeBlock {
                min: [-4, 8, -4],
                max: [4, 12, 4],
                material: 3,
            },
            RangeBlock {
                min: [-1, 0, -1],
                max: [1, 12, 1],
                material: 0,
            },
        ],
        units: vec![],
        unit_yaws: vec![],
        shields: vec![],
    }
}
#[test]
fn material_palette_does_not_change_the_stand_until_explicit_apply() {
    let mut lab = Laboratory::new(false);
    let recipe = lab.recipe;
    let (before, _) = lab.prepare().unwrap();
    let snapshot = before.snapshot();
    let revision = lab.constructor.revision;
    let (changed, _) = lab
        .constructor
        .action(LabAction::PickMaterial(5), &lab.definition, &mut lab.recipe)
        .unwrap();
    assert!(!changed);
    assert_eq!(lab.recipe, recipe);
    assert_eq!(lab.constructor.revision, revision);
    assert!(lab
        .constructor
        .material_context(&lab.definition, lab.recipe)
        .contains("Сейчас: Сталь"));
    assert_eq!(
        lab.prepare().unwrap().0.snapshot(),
        snapshot,
        "palette selection rebuilt the steel stand"
    );
    assert!(
        lab.constructor
            .action(LabAction::ApplyMaterial, &lab.definition, &mut lab.recipe)
            .unwrap()
            .0
    );
    let p = lab
        .definition
        .fixture(lab.recipe)
        .unwrap()
        .1
        .min
        .map(|v| v as u32);
    let (sim, _) = lab.prepare().unwrap();
    assert_eq!(sim.world.material(p[0], p[1], p[2]), 5);
}
#[test]
fn material_apply_changes_only_the_selected_assembly_preserving_cutouts_stand_and_library() {
    let mut lab = Laboratory::new(false);
    let original = assembly();
    let catalog = lab.constructor.catalog.clone();
    lab.constructor.placed = vec![
        catalog::Placed {
            element: original.clone(),
            at: [448, 16, 336],
            turns: 0,
        },
        catalog::Placed {
            element: original.clone(),
            at: [480, 16, 336],
            turns: 0,
        },
    ];
    lab.constructor
        .action(LabAction::PickObject(0), &lab.definition, &mut lab.recipe)
        .unwrap();
    assert_eq!(lab.constructor.material(&lab.definition, lab.recipe), 2);
    assert!(ui::enabled(&lab, LabAction::ApplyMaterial));
    lab.constructor
        .action(LabAction::PickMaterial(5), &lab.definition, &mut lab.recipe)
        .unwrap();
    assert!(ui::selected(&lab, LabAction::PickMaterial(5)));
    assert_eq!(
        lab.constructor.placed[0].element, original,
        "palette selection changed author geometry"
    );
    lab.constructor
        .action(LabAction::ApplyMaterial, &lab.definition, &mut lab.recipe)
        .unwrap();
    assert_eq!(
        lab.constructor.placed[0]
            .element
            .blocks
            .iter()
            .map(|b| b.material)
            .collect::<Vec<_>>(),
        [5, 5, 0]
    );
    assert_eq!(lab.constructor.placed[1].element, original);
    assert_eq!(lab.constructor.catalog, catalog);
    let (sim, _) = lab.prepare().unwrap();
    assert_eq!(sim.world.material(445, 20, 333), 5);
    assert_eq!(sim.world.material(448, 20, 336), 0);
    assert_eq!(sim.world.material(477, 20, 333), 2);
    let stand = lab
        .definition
        .fixture(lab.recipe)
        .unwrap()
        .1
        .min
        .map(|v| v as u32);
    assert_eq!(sim.world.material(stand[0], stand[1], stand[2]), 3);
    let file = SceneFile {
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
    let restored = SceneFile::from_ron(&file.to_ron().unwrap()).unwrap();
    assert_eq!(restored.workspace.selected_material, Some(5));
    assert_eq!(restored.elements, lab.constructor.placed);
}
#[test]
fn material_selection_refreshes_existing_targets_and_new_part_previews() {
    let mut lab = Laboratory::new(false);
    lab.constructor.stand = false;
    lab.constructor.placed = vec![catalog::Placed {
        element: assembly(),
        at: [448, 16, 336],
        turns: 0,
    }];
    lab.constructor.workspace.material = vec![(3, 0, 900), (2, 0, 120)];
    lab.constructor
        .action(LabAction::PickObject(0), &lab.definition, &mut lab.recipe)
        .unwrap();
    assert_eq!(parameters::get(&lab, 30), 120);
    lab.constructor
        .action(LabAction::PickMaterial(3), &lab.definition, &mut lab.recipe)
        .unwrap();
    assert_eq!(parameters::get(&lab, 30), 900);
    lab.constructor.mode = constructor::Mode::Part;
    assert!(!ui::enabled(&lab, LabAction::ApplyMaterial));
    assert!(lab
        .constructor
        .action(LabAction::ApplyMaterial, &lab.definition, &mut lab.recipe)
        .is_err());
    lab.constructor
        .preview_at([480, 16, 336], &lab.definition, lab.recipe);
    assert_eq!(
        lab.constructor.preview.as_ref().unwrap().geometry.blocks[0].material,
        3
    );
    lab.constructor
        .action(LabAction::PickMaterial(5), &lab.definition, &mut lab.recipe)
        .unwrap();
    lab.constructor
        .preview_at([480, 16, 336], &lab.definition, lab.recipe);
    assert_eq!(
        lab.constructor.preview.as_ref().unwrap().geometry.blocks[0].material,
        5
    );
    assert_eq!(lab.constructor.placed[0].element, assembly());
}
#[test]
fn material_apply_rejects_template_units_with_an_explicit_target_explanation() {
    let mut lab = Laboratory::new(false);
    lab.constructor.stand = false;
    let tank = lab
        .constructor
        .catalog
        .iter()
        .find(|e| e.id == "tank")
        .unwrap()
        .clone();
    lab.constructor.placed = vec![catalog::Placed {
        element: tank.clone(),
        at: [448, 16, 336],
        turns: 0,
    }];
    lab.constructor
        .action(LabAction::PickObject(0), &lab.definition, &mut lab.recipe)
        .unwrap();
    assert!(!ui::enabled(&lab, LabAction::ApplyMaterial));
    assert!(lab
        .constructor
        .material_context(&lab.definition, lab.recipe)
        .contains("шаблоном"));
    assert!(lab
        .constructor
        .action(LabAction::ApplyMaterial, &lab.definition, &mut lab.recipe)
        .is_err());
    assert_eq!(lab.constructor.placed[0].element, tank);
}

#[test]
fn material_tab_selects_the_just_placed_object_without_editing_a_stale_selection() {
    let d = Definition::stock();
    let mut r = Recipe::default();
    let mut c = constructor::Constructor::new();
    c.stand = false;
    c.action(LabAction::PickCatalog(0), &d, &mut r).unwrap();
    c.open_materials(&d);
    assert!(
        !c.can_apply_material(),
        "unplaced library preview edited another target"
    );
    c.action(LabAction::PlaceAt([448, 16, 336]), &d, &mut r)
        .unwrap();
    assert_eq!(c.mode, constructor::Mode::Place);
    c.open_materials(&d);
    assert_eq!(c.mode, constructor::Mode::Select);
    assert!(c.can_apply_material());
    c.action(LabAction::Duplicate, &d, &mut r).unwrap();
    c.open_materials(&d);
    assert!(!c.can_apply_material(), "unplaced copy edited its source");
    c.selected = Some(0);
    c.mode = constructor::Mode::Select;
    c.action(LabAction::Place, &d, &mut r).unwrap();
    c.open_materials(&d);
    assert!(!c.can_apply_material(), "placement tool kept an old target");
}
