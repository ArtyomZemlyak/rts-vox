//! Сохранение партии — **проверкой, а не обещанием** (ADR-229).
//!
//! Одна проверка здесь главная: прогон `N` тиков, сохранение, загрузка
//! и ещё `M` тиков обязаны дать **тот же хеш**, что непрерывный прогон
//! `N + M` тиков. Всё остальное в файле — подпорки к ней.
//!
//! Сценарий тот же, что у golden-хеша: эталонный, с землёй, копкой,
//! боем и подкреплениями. Сохранение, проверенное на пустом мире,
//! ничего не проверяет — ровно та же болезнь, что ADR-116 и ADR-157.

use sim_core::save::{load, load_with, save, LoadError};
use sim_core::{Order, Reference};

const SEED: u64 = 42;
const UNITS: usize = 1200;

fn tuning() -> sim_core::Tuning {
    sim_core::Tuning::from_ron(include_str!("../../../data/sim.ron")).expect("data/sim.ron")
}

fn weapons() -> modding::weapons::Weapons {
    modding::weapons::from_ron(include_str!("../../../data/weapons.ron")).expect("data/weapons.ron")
}

fn reference() -> Reference {
    Reference::new(SEED, UNITS, tuning(), weapons())
}

fn kinds() -> modding::Kinds {
    modding::kinds::from_ron(
        include_str!("../../../data/kinds.ron"),
        sim_core::TICKS_PER_SECOND as u32,
    )
    .expect("data/kinds.ron")
}

fn mats() -> modding::Registry {
    modding::data::materials_from_ron(include_str!("../../../data/materials.ron"))
        .expect("data/materials.ron")
}

/// Номера типов из `data/kinds.ron`. Явные, как везде (ADR-028).
const FACTORY: u16 = 10;
const TANK: u16 = 1;

/// Эталонный сценарий **с экономикой**: завод у каждой стороны,
/// заказ на танк и приказ строить склад рядом.
///
/// Экономика не входит в эталонный прогон (ADR-230), и сценарий
/// сохранения без неё проверял бы формат, которого не касается
/// половина полей, — та же болезнь, что ADR-116.
fn with_economy() -> Reference {
    let mut r = Reference::new(SEED, UNITS / 4, tuning(), weapons());
    r.world.attach_economy(kinds(), mats());
    let (ex, ez) = r.world.ground().expect("земля").extent();
    for (owner, dx) in [(0u8, 1), (1u8, 3)] {
        let (x, z) = (ex * dx / 4, ez / 2);
        let Some(f) = r.world.spawn_kind(x, z, owner, FACTORY) else {
            panic!("завод стороны {owner} не встал");
        };
        r.world.push_order(Order::Produce {
            unit: f,
            kind: TANK,
        });
        r.world.push_order(Order::Produce {
            unit: f,
            kind: TANK,
        });
    }
    r
}

/// **Сохранил, загрузил, продолжил — тот же хеш.**
///
/// Ради этого всё и написано. Сравнение потиковое, а не только
/// по итогу: расхождение, найденное на сотом тике из ста, ничего
/// не говорит о том, где оно возникло.
#[test]
fn a_loaded_game_continues_bit_for_bit() {
    const N: usize = 300;
    const M: usize = 200;

    let mut straight = reference();
    for _ in 0..N {
        straight.tick();
    }
    let want: Vec<u64> = (0..M).map(|_| straight.tick()).collect();

    let mut cut = reference();
    for _ in 0..N {
        cut.tick();
    }
    let blob = save(&cut);
    let mut back = load(&blob, tuning(), weapons()).expect("сохранение не прочиталось");

    for (k, w) in want.iter().enumerate() {
        let got = back.tick();
        assert_eq!(
            got, *w,
            "расхождение на {k}-м тике после загрузки: {got:016x} вместо {w:016x}"
        );
    }
}

/// **Партия с экономикой продолжается тем же битом.**
///
/// Баки, склады, очередь завода и недострои — состояние, и потеряй
/// их файл, загруженная партия разошлась бы не сразу, а на первом
/// достроенном заказе. Потому сравнение потиковое и после загрузки,
/// а не по итогу.
#[test]
fn a_loaded_game_with_economy_continues_bit_for_bit() {
    const N: usize = 120;
    const M: usize = 80;

    let mut straight = with_economy();
    for _ in 0..N {
        straight.tick();
    }
    let want: Vec<u64> = (0..M).map(|_| straight.tick()).collect();

    let mut cut = with_economy();
    for _ in 0..N {
        cut.tick();
    }
    // Сценарий обязан нагружать то, что декларирует: к точке сохранения
    // экономика должна что-то сделать, иначе проверять нечего.
    {
        let e = cut.world.econ.as_ref().expect("экономика");
        assert!(
            e.ledger.live_containers() > 2,
            "к точке сохранения контейнеров всего {}",
            e.ledger.live_containers()
        );
        let (_, lines, _, _) = e.parts();
        assert!(
            !lines.is_empty(),
            "к точке сохранения заводы ничего не делают"
        );
    }
    let blob = save(&cut);
    let mut back = load_with(&blob, tuning(), weapons(), Some((kinds(), mats())))
        .expect("сохранение не прочиталось");

    for (k, w) in want.iter().enumerate() {
        let got = back.tick();
        assert_eq!(
            got, *w,
            "расхождение на {k}-м тике после загрузки: {got:016x} вместо {w:016x}"
        );
    }
}

/// **Партия с экономикой не читается без таблицы типов.**
///
/// Типы приходят из данных, и открыть такую партию без них значит
/// молча получить партию без построек — то есть другую.
#[test]
fn a_game_with_economy_needs_its_kinds() {
    let mut r = with_economy();
    for _ in 0..30 {
        r.tick();
    }
    let blob = save(&r);
    assert!(
        matches!(
            load(&blob, tuning(), weapons()),
            Err(LoadError::WorldMismatch)
        ),
        "партия с экономикой прочиталась без таблицы типов"
    );
}

/// **Сценарий обязан нагружать то, что декларирует** (`CLAUDE.md`).
///
/// Проверка выше пройдёт и на мире, в котором за триста тиков ничего
/// не произошло: нечему расходиться — нечему и совпадать. Здесь
/// проверяется, что к точке сохранения мир уже правлен, тела падали,
/// снаряды летали и кто-то погиб.
#[test]
fn the_save_point_is_not_an_empty_world() {
    let mut r = reference();
    for _ in 0..300 {
        r.tick();
    }
    let g = r.world.ground().expect("у эталона нет земли");
    assert!(
        g.world.touched_chunks() > 0,
        "слой правок пуст: сохранять нечего"
    );
    assert!(r.world.deaths > 0, "никто не погиб — боя в точке среза нет");
    assert!(
        r.world.shots_fired > 0,
        "не стреляли — снарядов в состоянии нет"
    );
    assert!(
        r.world.flow_solves > 0,
        "поле направлений не опубликовано — сохранять нечего"
    );
}

/// **Загруженный мир равен исходному по состоянию, а не только
/// по дальнейшему ходу.**
///
/// Хеш тика видит юнитов, снаряды и поток правок — но не содержимое
/// мира (это замечено ещё в ADR-217). Поэтому мир сверяется отдельно:
/// материал по разреженной решётке обязан совпасть до воксела.
#[test]
fn the_loaded_world_has_the_same_voxels() {
    let mut r = reference();
    for _ in 0..200 {
        r.tick();
    }
    let back = load(&save(&r), tuning(), weapons()).expect("сохранение не прочиталось");
    let a = r.world.ground().expect("земля").world.nx;
    let ga = &r.world.ground().expect("земля").world;
    let gb = &back.world.ground().expect("земля").world;
    assert_eq!(
        gb.touched_chunks(),
        ga.touched_chunks(),
        "тронуто не столько же"
    );
    let mut same = 0u64;
    for z in (0..ga.nz).step_by(3) {
        for y in (0..ga.ny).step_by(3) {
            for x in (0..a).step_by(3) {
                assert_eq!(
                    ga.material(x, y, z),
                    gb.material(x, y, z),
                    "материал разошёлся в ({x}, {y}, {z})"
                );
                assert_eq!(
                    ga.solid(x, y, z),
                    gb.solid(x, y, z),
                    "занятость ({x}, {y}, {z})"
                );
                same += 1;
            }
        }
    }
    assert!(
        same > 100_000,
        "сверено всего {same} вокселей — решётка редка"
    );
}

/// **Сохранение сохранённого — тот же файл.**
///
/// Порядок обхода при записи фиксирован (`CLAUDE.md`), значит запись
/// одного и того же состояния обязана давать один и тот же байт.
/// Разойдись — и сравнивать сохранения между клиентами было бы нечем.
#[test]
fn saving_a_loaded_game_gives_the_same_bytes() {
    let mut r = reference();
    for _ in 0..150 {
        r.tick();
    }
    let first = save(&r);
    let back = load(&first, tuning(), weapons()).expect("сохранение не прочиталось");
    let second = save(&back);
    assert_eq!(first.len(), second.len(), "длина файла разошлась");
    assert!(first == second, "запись загруженного дала другие байты");
}

/// **Числа тика в сохранении не заморожены.**
///
/// Потолки и тяготение — правила, по которым идёт партия, а не её
/// состояние: они живут в `data/*.ron` (ADR-140). Запиши их в файл —
/// и правка `sim.ron` перестала бы действовать на загруженную партию.
/// Проверяется прямо: загрузка с изменёнными числами обязана взять
/// изменённые.
#[test]
fn the_save_does_not_freeze_the_tuning() {
    let small = worldgen::Params::sized(64, 64, 64);
    let mut r = Reference::with_params(SEED, 200, tuning(), weapons(), small);
    for _ in 0..20 {
        r.tick();
    }
    let blob = save(&r);
    let mut t = tuning();
    t.max_bodies += 7;
    t.max_materialize_per_tick += 1;
    let back = load(&blob, t, weapons()).expect("сохранение не прочиталось");
    let g = back.world.ground().expect("земля");
    assert_eq!(
        g.bodies.max_bodies, t.max_bodies,
        "потолок тел взят из файла, а не из data/sim.ron"
    );
    assert_eq!(
        g.world.max_materialize_per_tick, t.max_materialize_per_tick as usize,
        "потолок материализаций взят из файла, а не из data/sim.ron"
    );
    assert_eq!(
        g.bodies.gravity,
        sim_core::gravity_per_tick(&t),
        "тяготение взято из файла, а не из data/sim.ron"
    );
}

/// Версия стоит в первых байтах и читается **до** разбора тела.
#[test]
fn the_version_is_in_the_first_bytes() {
    let r = reference();
    let mut blob = save(&r);
    assert_eq!(&blob[..8], b"RTSVOXSV");
    assert_eq!(
        u32::from_le_bytes([blob[8], blob[9], blob[10], blob[11]]),
        10
    );
    blob[8] = 99;
    assert!(matches!(
        load(&blob, tuning(), weapons()),
        Err(LoadError::Version {
            found: 99,
            want: 10
        })
    ));
}

#[test]
fn a_material_body_and_unfinished_impact_survive_a_full_game_save() {
    use fx::Fx;
    let mut t = tuning();
    t.impact.cells_per_tick = 1;
    let mut r = Reference::new(SEED, 0, t, weapons());
    r.world.attach_economy(kinds(), mats());
    let id = r.world.spawn_kind(300, 300, 0, TANK).unwrap();
    r.world.units.refresh_live();
    r.world.push_order(Order::Hold { unit: id });
    let bodies = r.world.material_bodies.as_mut().unwrap();
    let body = bodies.body(id).unwrap();
    let core = body.core;
    let a = [
        Fx::from_int(-1),
        Fx::from_int(core.1 as i32).add(Fx::from_ratio(1, 2)),
        Fx::from_int(core.2 as i32).add(Fx::from_ratio(1, 2)),
    ];
    let b = [Fx::from_int(body.vol.nx as i32 + 1), a[1], a[2]];
    assert!(bodies.queue(id, a, b, 9_000_000_000, t.impact.max_pending as usize));
    bodies.step(&t.impact.work_kj_m3, 2, 1);
    assert!(!bodies.pending.is_empty());
    let blob = save(&r);
    let mut back = load_with(&blob, t, weapons(), Some((kinds(), mats()))).unwrap();
    assert_eq!(
        r.world.material_bodies.as_ref().unwrap().hash(),
        back.world.material_bodies.as_ref().unwrap().hash()
    );
    for tick in 0..45 {
        assert_eq!(r.tick(), back.tick(), "material resume diverged at {tick}");
        assert!(r.world.econ.as_ref().unwrap().balanced());
    }
    assert!(
        r.world
            .material_bodies
            .as_ref()
            .unwrap()
            .state(id)
            .unwrap()
            .revision
            > 0
    );
}

#[test]
fn a_blast_builder_and_flying_matter_survive_a_full_game_save() {
    let mut t = tuning();
    t.blast.samples_per_tick = 5000;
    let mut r = Reference::new(SEED, 0, t, weapons());
    r.world.attach_economy(kinds(), mats());
    let g = r.world.ground.as_ref().unwrap();
    let y = (1..g.world.ny)
        .rev()
        .find(|&y| g.world.solid(300, y, 300))
        .unwrap() as i32;
    assert!(r
        .world
        .bursts
        .enqueue(&g.world, [300, y + 1, 300], 12, 120_000, &t.blast));
    for cut in [1, 90] {
        for _ in 0..cut {
            r.world.tick();
        }
        let bytes = save(&r);
        let mut back = load_with(&bytes, t, weapons(), Some((kinds(), mats()))).unwrap();
        assert_eq!(r.world.bursts.hash(), back.world.bursts.hash());
        for tick in 0..45 {
            assert_eq!(
                r.world.tick(),
                back.world.tick(),
                "burst resume at {cut}/{tick}"
            );
            assert!(r.world.econ.as_ref().unwrap().balanced());
        }
    }
    assert!(r.world.bursts.removed > 0);
}

#[test]
fn a_frozen_unit_wave_and_coloured_flying_armour_survive_a_game_save() {
    let mut t = tuning();
    t.blast.samples_per_tick = 5000;
    t.blast.edits_per_tick = 211;
    let mut r = Reference::new(SEED, 0, t, weapons());
    r.world.attach_economy(kinds(), mats());
    let id = r.world.spawn_kind(300, 300, 0, TANK).unwrap();
    r.world.units.refresh_live();
    r.world.push_order(Order::Hold { unit: id });
    r.world.tick();
    let bodies = r.world.material_bodies.as_ref().unwrap();
    let frames = bodies.frames(&r.world.units);
    let frame = frames[id as usize].unwrap()[0];
    let body = bodies.body(id).unwrap();
    let mut hit = None;
    'scan: for z in 0..body.vol.nz {
        for y in 0..body.vol.ny {
            for x in 0..body.vol.nx {
                if body.vol.get(x as usize, y as usize, z as usize) {
                    hit = Some([x, y, z]);
                    break 'scan;
                }
            }
        }
    }
    let source = frame.world(
        hit.unwrap()
            .map(|v| fx::Fx::from_int(v as i32).add(fx::Fx::from_ratio(1, 2))),
    );
    let at = [source.x, source.y, source.z].map(|v| v.to_int_trunc());
    assert!(r.world.bursts.enqueue(
        &r.world.ground.as_ref().unwrap().world,
        at,
        40,
        6_000_000,
        &t.blast
    ));
    r.world.bursts.capture_last(bodies, &frames, &t.blast);
    for cut in [1, 100] {
        for _ in 0..cut {
            r.world.tick();
        }
        let bytes = save(&r);
        let mut back = load_with(&bytes, t, weapons(), Some((kinds(), mats()))).unwrap();
        assert_eq!(bytes, save(&back));
        for tick in 0..100 {
            assert_eq!(
                r.world.tick(),
                back.world.tick(),
                "unit field resume {cut}/{tick}"
            );
        }
    }
    assert!(
        r.world.bursts.body_removed > 0,
        "pending {} damage {} revision {}",
        r.world.bursts.pending(),
        r.world.material_bodies.as_ref().unwrap().pending.len(),
        r.world
            .material_bodies
            .as_ref()
            .unwrap()
            .state(id)
            .unwrap()
            .revision
    );
    assert!(r.world.bursts.fragments.iter().any(|f| f.paint.is_some()));
    assert!(r.world.econ.as_ref().unwrap().balanced());
}

#[test]
fn a_queued_attack_move_and_its_firing_stop_resume_from_a_save() {
    let t = tuning();
    let mut r = Reference::new(SEED, 0, t, weapons());
    r.world.attach_economy(kinds(), mats());
    let a = r.world.spawn_kind(300, 300, 0, TANK).unwrap();
    let b = r.world.spawn_kind(500, 300, 1, TANK).unwrap();
    r.world.units.refresh_live();
    r.world.push_order(Order::Hold { unit: b });
    r.world.push_order(Order::AttackMove {
        unit: a,
        x: 600,
        z: 300,
    });
    for cut in [0, 30] {
        for _ in 0..cut {
            r.world.tick();
        }
        let bytes = save(&r);
        let mut back = load_with(&bytes, t, weapons(), Some((kinds(), mats()))).unwrap();
        assert_eq!(bytes, save(&back));
        for tick in 0..30 {
            assert_eq!(
                r.world.tick(),
                back.world.tick(),
                "engagement resume {cut}/{tick}"
            );
        }
    }
    assert_eq!(r.world.units.stance[a as usize], sim_core::STANCE_ENGAGED);
    assert!(r.world.units.x[a as usize] < 400);
}

#[test]
fn an_unfinished_separation_and_its_whole_section_resume_from_a_game_save() {
    let mut t = tuning();
    t.impact.detach_work_per_tick = 1;
    let mut r = Reference::new(SEED, 0, t, weapons());
    r.world.attach_economy(kinds(), mats());
    let id = r.world.spawn_kind(300, 300, 0, TANK).unwrap();
    r.world.units.refresh_live();
    r.world.push_order(Order::Hold { unit: id });
    r.world.tick();
    let held = r.world.units.store[id as usize];
    let before = r.world.econ.as_ref().unwrap().ledger.held(held);
    let bodies = r.world.material_bodies.as_mut().unwrap();
    let body = bodies.body(id).unwrap();
    let plane = body.turret_y - 1;
    assert!(body.turret_y > 0 && body.core.1 < plane);
    let mut cells = Vec::new();
    for z in 0..body.vol.nz {
        for x in 0..body.vol.nx {
            if body.vol.get(x as usize, plane as usize, z as usize) {
                cells.push([x, plane, z]);
            }
        }
    }
    let cut = cells.len();
    let generation = bodies.generations[id as usize];
    for batch in cells.chunks(768) {
        assert!(bodies.queue_cells(
            id,
            generation,
            batch.to_vec(),
            u64::MAX / 4,
            0,
            bodies.frames(&r.world.units)[id as usize].unwrap()[0]
                .x
                .scale(fx::Fx::ZERO),
            t.impact.max_pending
        ));
    }
    r.world.tick();
    assert!(r.world.material_bodies.as_ref().unwrap().detach_pending() > 0);
    let bytes = save(&r);
    let mut back = load_with(&bytes, t, weapons(), Some((kinds(), mats()))).unwrap();
    assert_eq!(bytes, save(&back));
    r.world.tuning.impact.detach_work_per_tick = 8192;
    back.world.tuning.impact.detach_work_per_tick = 8192;
    for tick in 0..60 {
        assert_eq!(
            r.world.tick(),
            back.world.tick_par(4),
            "separation resume {tick}"
        );
    }
    let b = r.world.material_bodies.as_ref().unwrap();
    assert_eq!(b.detach_pending(), 0);
    assert!(b.state(id).unwrap().status.alive);
    assert!(!b.state(id).unwrap().status.weapon);
    let chunks: Vec<_> = r
        .world
        .bursts
        .fragments
        .iter()
        .filter(|f| f.chunk.is_some())
        .collect();
    assert!(!chunks.is_empty());
    assert!(chunks.iter().any(|f| f.count > 8));
    assert_eq!(
        b.state(id).unwrap().removed.len(),
        cut + chunks.iter().map(|f| f.count as usize).sum::<usize>()
    );
    let e = r.world.econ.as_ref().unwrap();
    assert_eq!(
        e.ledger.held(held)
            + r.world
                .bursts
                .fragments
                .iter()
                .map(|f| e.ledger.held(f.held))
                .sum::<u64>(),
        before
    );
    assert!(e.balanced());
    let bytes = save(&r);
    let mut back = load_with(&bytes, r.world.tuning, weapons(), Some((kinds(), mats()))).unwrap();
    assert_eq!(bytes, save(&back));
    for tick in 0..30 {
        assert_eq!(
            r.world.tick(),
            back.world.tick_par(4),
            "whole section resume {tick}"
        );
    }
}

#[test]
fn a_damaged_dead_hull_keeps_its_dents_matter_and_falling_pose_after_load() {
    let t = tuning();
    let mut r = Reference::new(SEED, 0, t, weapons());
    r.world.attach_economy(kinds(), mats());
    let id = r.world.spawn_kind(300, 300, 0, TANK).unwrap();
    r.world.units.refresh_live();
    r.world.push_order(Order::Hold { unit: id });
    r.world.tick();
    let bodies = r.world.material_bodies.as_mut().unwrap();
    let body = bodies.body(id).unwrap();
    let mut cells = Vec::new();
    'scan: for z in 0..body.vol.nz {
        for y in 0..body.vol.ny {
            for x in 0..body.vol.nx {
                if body.vol.get(x as usize, y as usize, z as usize)
                    && body.part_at(x as usize, y as usize, z as usize) == voxel::Part::Armour
                {
                    cells.push([x, y, z]);
                    if cells.len() == 2 {
                        break 'scan;
                    }
                }
            }
        }
    }
    let dose = t.impact.work_kj_m3[0] as u64 * (body.mm as u64).pow(3) / 1000;
    let direction = bodies.frames(&r.world.units)[id as usize].unwrap()[0].x;
    assert!(bodies.queue_cells(
        id,
        bodies.generations[id as usize],
        cells,
        dose + dose / 2,
        0,
        direction,
        t.impact.max_pending
    ));
    r.world.tick();
    assert!(!r
        .world
        .material_bodies
        .as_ref()
        .unwrap()
        .state(id)
        .unwrap()
        .dents
        .is_empty());
    r.world.units.y[id as usize] = r.world.ground.as_ref().unwrap().world.ny as i32 + 40;
    r.world.units.voxels[id as usize] = 0;
    r.world.tick();
    assert_eq!(r.world.bursts.corpses.len(), 1);
    let held = *r.world.bursts.corpses.keys().next().unwrap();
    let source = &r.world.bursts.corpses[&held];
    assert!(!source.resting);
    let initial_y = source.position.y;
    let original_shape = source.chunk.as_ref().unwrap().hash();
    assert!(!source.chunk.as_ref().unwrap().dents.is_empty());
    let bytes = save(&r);
    let mut back = load_with(&bytes, t, weapons(), Some((kinds(), mats()))).unwrap();
    assert_eq!(bytes, save(&back));
    for tick in 0..180 {
        assert_eq!(
            r.world.tick(),
            back.world.tick_par(4),
            "dead hull fall {tick}"
        );
    }
    let f = &r.world.bursts.corpses[&held];
    assert!(f.resting && f.position.y < initial_y);
    assert_eq!(original_shape, f.chunk.as_ref().unwrap().hash());
    assert_eq!(
        r.world
            .econ
            .as_ref()
            .unwrap()
            .wrecks
            .iter()
            .filter(|w| w.held == held)
            .count(),
        1
    );
    assert!(r.world.econ.as_ref().unwrap().balanced());
    let bytes = save(&r);
    let back = load_with(&bytes, t, weapons(), Some((kinds(), mats()))).unwrap();
    assert_eq!(bytes, save(&back));
    r.world.bursts.corpses.get_mut(&held).unwrap().held = u32::MAX - 1;
    assert!(load_with(&save(&r), t, weapons(), Some((kinds(), mats()))).is_err());
}

#[test]
fn damaged_mass_and_partial_engine_resume_the_same_manoeuvre() {
    let t = tuning();
    let mut r = Reference::new(SEED, 0, t, weapons());
    r.world.attach_economy(kinds(), mats());
    let id = r.world.spawn_kind(300, 300, 0, TANK).unwrap();
    r.world.units.refresh_live();
    r.world.push_order(Order::MoveTo {
        unit: id,
        x: 600,
        z: 340,
    });
    r.world.tick();
    let b = r.world.material_bodies.as_mut().unwrap();
    let body = b.body(id).unwrap();
    let mut engines = Vec::new();
    let mut cells = Vec::new();
    for z in 0..body.vol.nz {
        for y in 0..body.vol.ny {
            for x in 0..body.vol.nx {
                if !body.vol.get(x as usize, y as usize, z as usize) {
                    continue;
                }
                let part = body.part_at(x as usize, y as usize, z as usize);
                if part == voxel::Part::Engine {
                    engines.push([x, y, z]);
                }
                if part == voxel::Part::Armour && y >= body.turret_y && x < body.core.0 {
                    cells.push([x, y, z]);
                }
            }
        }
    }
    assert!(engines.len() > 8);
    cells.extend(engines.into_iter().step_by(4));
    let dir = b.frames(&r.world.units)[id as usize].unwrap()[0].x;
    for batch in cells.chunks(768) {
        assert!(b.queue_cells(
            id,
            b.generations[id as usize],
            batch.to_vec(),
            u64::MAX / 8,
            0,
            dir,
            t.impact.max_pending
        ));
    }
    r.world.tick();
    let response = r
        .world
        .material_bodies
        .as_ref()
        .unwrap()
        .state(id)
        .unwrap()
        .mobility;
    assert!(response.inverse_mass > fx::Fx::ONE);
    assert!(response.engine > fx::Fx::ZERO && response.engine < fx::Fx::ONE);
    assert!(r.world.units.alive[id as usize]);
    let bytes = save(&r);
    let mut back = load_with(&bytes, t, weapons(), Some((kinds(), mats()))).unwrap();
    assert_eq!(bytes, save(&back));
    assert_eq!(
        response,
        back.world
            .material_bodies
            .as_ref()
            .unwrap()
            .state(id)
            .unwrap()
            .mobility
    );
    let old = (r.world.units.x[id as usize], r.world.units.z[id as usize]);
    for tick in 0..90 {
        assert_eq!(
            r.world.tick(),
            back.world.tick_par(4),
            "damaged drive resume {tick}"
        );
    }
    assert_ne!(
        old,
        (r.world.units.x[id as usize], r.world.units.z[id as usize])
    );
    assert!(r.world.econ.as_ref().unwrap().balanced());
}

#[test]
fn a_walker_resumes_its_planted_turn_and_cadence_after_loading() {
    let t = tuning();
    let mut r = Reference::new(SEED, 0, t, weapons());
    r.world.attach_economy(kinds(), mats());
    let id = r.world.spawn_kind(300, 300, 0, 3).unwrap();
    r.world.units.refresh_live();
    r.world.push_order(Order::MoveTo {
        unit: id,
        x: 100,
        z: 350,
    });
    for _ in 0..5 {
        r.world.tick();
    }
    let i = id as usize;
    assert_ne!(r.world.units.gait[i], 0);
    assert_ne!(r.world.units.yaw_rate[i], 0);
    assert_eq!(r.world.units.vel[i], fx::Fx::ZERO);
    let bytes = save(&r);
    let mut back = load_with(&bytes, t, weapons(), Some((kinds(), mats()))).unwrap();
    assert_eq!(bytes, save(&back));
    for tick in 0..120 {
        assert_eq!(
            r.world.tick(),
            back.world.tick_par(4),
            "pivot resume {tick}"
        );
    }
    assert!(r.world.econ.as_ref().unwrap().balanced());
}

#[test]
fn a_moving_suspension_resumes_and_the_collision_frame_uses_its_height() {
    let t = tuning();
    let mut params = worldgen::Params::sized(192, 128, 192);
    params.relief = 1;
    let mut r = Reference::with_params(SEED, 0, t, weapons(), params);
    r.world.attach_economy(kinds(), mats());
    let id = r.world.spawn_kind(96, 96, 0, 3).unwrap();
    let i = id as usize;
    r.world.units.refresh_live();
    r.world.push_order(Order::Hold { unit: id });
    r.world.tick();
    let level = r.world.units.y[i];
    let initial = r.world.units.body_y(i);
    let ground = r.world.ground_mut().unwrap();
    for x in 72..120 {
        for z in 72..120 {
            for y in level - 4..level {
                ground.world.queue_dig(x, y as u32, z);
            }
        }
    }
    r.world.tick();
    assert_eq!(r.world.units.y[i], level - 4);
    assert!(
        r.world.units.body_y(i) > initial.sub(fx::Fx::ONE),
        "body snapped to the new navigation floor"
    );
    assert!(r.world.units.heave_v[i] < fx::Fx::ZERO);
    let frames = r
        .world
        .material_bodies
        .as_ref()
        .unwrap()
        .frames(&r.world.units);
    assert_eq!(frames[i].unwrap()[0].base.y, r.world.units.body_y(i));
    let bytes = save(&r);
    let mut back = load_with(&bytes, t, weapons(), Some((kinds(), mats()))).unwrap();
    assert_eq!(bytes, save(&back));
    for tick in 0..150 {
        assert_eq!(
            r.world.tick(),
            back.world.tick_par(4),
            "suspension resume {tick}"
        );
    }
    assert!(
        r.world.units.heave[i].abs() < fx::Fx::from_ratio(1, 20),
        "suspension did not settle"
    );
    assert!(r.world.units.heave_v[i].abs() < fx::Fx::from_ratio(1, 20));
    r.world.units.kill(id);
    let reused = r.world.units.spawn(20, 20, 0, 30, 100) as usize;
    assert_eq!(reused, i);
    assert_eq!(r.world.units.heave[i], fx::Fx::ZERO);
    assert_eq!(r.world.units.heave_v[i], fx::Fx::ZERO);
}

#[test]
fn a_hole_between_supports_does_not_create_a_fall_but_losing_them_does() {
    use fx::Fx;
    let t = tuning();
    let mut params = worldgen::Params::sized(192, 128, 192);
    params.relief = 1;
    let mut r = Reference::with_params(SEED, 0, t, weapons(), params);
    r.world.attach_economy(kinds(), mats());
    let id = r.world.spawn_kind(96, 96, 0, 3).unwrap();
    let i = id as usize;
    r.world.units.refresh_live();
    r.world.push_order(Order::Hold { unit: id });
    r.world.tick();
    let top = r.world.units.y[i];
    let initial = r.world.units.body_y(i);
    let voxels = r.world.units.voxels[i];
    let dig = |r: &mut Reference, lo: u32, hi: u32| {
        let g = r.world.ground_mut().unwrap();
        for x in lo..hi {
            for z in lo..hi {
                for y in 20..top as u32 {
                    g.world.queue_dig(x, y, z);
                }
            }
        }
    };
    dig(&mut r, 90, 100);
    for _ in 0..45 {
        r.world.tick();
        assert_eq!(
            r.world.units.fall_from[i],
            sim_core::NOT_FALLING,
            "standing supports were ignored"
        );
        assert!(r.world.units.body_y(i).sub(initial).abs() < Fx::from_ratio(1, 100));
        assert_eq!(
            r.world.units.voxels[i], voxels,
            "navigation manufactured landing damage"
        );
    }
    assert_eq!(r.world.units.y[i], 20);
    dig(&mut r, 80, 112);
    r.world.tick();
    let first_drop = initial.sub(r.world.units.body_y(i));
    assert!(
        first_drop > Fx::from_ratio(1, 10) && first_drop < Fx::from_ratio(12, 100),
        "gravity was integrated twice or the body teleported: {first_drop:?}"
    );
    assert_ne!(r.world.units.fall_from[i], sim_core::NOT_FALLING);
    for _ in 0..8 {
        r.world.tick();
    }
    let bytes = save(&r);
    let mut back = load_with(&bytes, t, weapons(), Some((kinds(), mats()))).unwrap();
    assert_eq!(save(&back), bytes);
    let mut landed = false;
    for tick in 0..180 {
        assert_eq!(
            r.world.tick(),
            back.world.tick_par(4),
            "unsupported body resume {tick}"
        );
        landed |= r.world.units.fall_from[i] == sim_core::NOT_FALLING;
    }
    assert!(landed);
    assert!(
        r.world.units.voxels[i] < voxels,
        "actual landing caused no damage"
    );
    assert!(r.world.units.body_y(i).sub(Fx::from_int(20)).abs() < Fx::from_ratio(1, 20));
    assert!(r.world.units.body_vy(i).abs() < Fx::from_ratio(1, 20));
    assert!(r.world.econ.as_ref().unwrap().balanced());
}

#[test]
fn version_nine_flight_migrates_position_and_velocity_without_a_jump() {
    use fx::Fx;
    let t = tuning();
    let mut r = Reference::new(SEED, 0, t, weapons());
    let id = r.world.units.spawn(90, 90, 0, 30, 300) as usize;
    r.world.units.refresh_live();
    r.world.units.y[id] = 90;
    r.world.units.heave[id] = Fx::from_ratio(5, 4);
    r.world.units.fall_from[id] = 100;
    r.world.units.fall_f[id] = Fx::from_ratio(3, 4);
    r.world.units.fall_v[id] = Fx::from_ratio(1, 2);
    r.world.units.heave_v[id] = Fx::ZERO;
    let body = r.world.units.body_y(id);
    let mut bytes = save(&r);
    bytes[8..12].copy_from_slice(&9u32.to_le_bytes());
    // v9 and v10 arrays have the same layout; only ownership of flight changes.
    let mut back = load(&bytes, t, weapons()).unwrap();
    assert_eq!(back.world.units.body_y(id), body);
    assert_eq!(back.world.units.body_vy(id), Fx::from_int(-15));
    assert_eq!(back.world.units.fall_f[id], Fx::ZERO);
    let current = save(&back);
    let mut again = load(&current, t, weapons()).unwrap();
    assert_eq!(current, save(&again));
    for tick in 0..90 {
        assert_eq!(
            back.world.tick(),
            again.world.tick_par(4),
            "migrated flight {tick}"
        );
    }
}
