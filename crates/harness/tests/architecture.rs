//! Правила из `docs/02-architecture.md` и `CLAUDE.md`, проверяемые
//! механически.
//!
//! Смысл файла — в том, что эти правила **объявлены как условие на CI,
//! а не как пожелание**, и до сих пор жили только в тексте. Правило,
//! которое никто не проверяет, соблюдается ровно до первой спешки.
//!
//! Разбор `Cargo.toml` здесь нарочно примитивный: нужны имена секций
//! и имена зависимостей, а не полный TOML. Зависимость от парсера ради
//! четырёх правил дороже самих правил.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

struct Manifest {
    name: String,
    /// Зависимости по пути — внутренние крейты.
    local: Vec<String>,
    /// Зависимости по имени — внешние.
    external: Vec<(String, String)>,
}

/// Прочитать манифест. Разбирается только секция `[dependencies]`:
/// строки вида `имя = ...` до следующей секции.
fn read_manifest(dir: &Path) -> Manifest {
    let text = std::fs::read_to_string(dir.join("Cargo.toml")).expect("Cargo.toml");
    let mut name = String::new();
    let mut local = Vec::new();
    let mut external = Vec::new();
    let mut in_deps = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_deps = t == "[dependencies]" || t == "[dev-dependencies]";
            continue;
        }
        if !in_deps {
            if let Some(v) = t.strip_prefix("name = ") {
                if name.is_empty() {
                    name = v.trim_matches('"').to_string();
                }
            }
            continue;
        }
        let Some((k, v)) = t.split_once('=') else {
            continue;
        };
        let (k, v) = (k.trim(), v.trim());
        if k.is_empty() || k.starts_with('#') {
            continue;
        }
        if v.contains("path =") {
            local.push(k.to_string());
        } else {
            external.push((k.to_string(), v.to_string()));
        }
    }
    Manifest {
        name,
        local,
        external,
    }
}

fn all_manifests() -> BTreeMap<String, Manifest> {
    let mut out = BTreeMap::new();
    let dir = root().join("crates");
    let mut names: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("crates/")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir())
        .collect();
    names.sort();
    for p in names {
        let m = read_manifest(&p);
        out.insert(m.name.clone(), m);
    }
    out
}

/// Крейты симуляции: те, от которых `sim-core` зависит **по
/// архитектуре**, а не только по сегодняшнему `Cargo.toml`.
///
/// Разница существенна. Сейчас `sim-core` подключает один `fx`,
/// и проверка по фактическому замыканию была бы почти пустой:
/// она зеленела бы ровно до дня, когда крейты свяжут как объявлено
/// (`docs/02-architecture.md`), и в тот же день посыпалась бы.
/// Правило — про архитектуру, а не про текущую проводку.
fn sim_closure(ms: &BTreeMap<String, Manifest>) -> Vec<String> {
    let mut seen: Vec<String> = ["sim-core", "fx", "voxel", "worldgen", "nav", "modding"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let mut i = 0;
    while i < seen.len() {
        let cur = seen[i].clone();
        i += 1;
        let Some(m) = ms.get(&cur) else { continue };
        for d in &m.local {
            if !seen.contains(d) {
                seen.push(d.clone());
            }
        }
    }
    seen.sort();
    seen
}

/// **Ни один крейт, от которого зависит `sim-core`, не имеет `bevy`,
/// `wgpu` или `winit`.** Проверка именно по трём именам: рендерная
/// зависимость просачивается через `wgpu` так же легко, как через Bevy.
#[test]
fn the_simulation_never_reaches_a_render_dependency() {
    let ms = all_manifests();
    let banned = ["bevy", "wgpu", "winit"];
    for c in sim_closure(&ms) {
        let m = &ms[&c];
        for (dep, _) in &m.external {
            assert!(
                !banned
                    .iter()
                    .any(|b| dep == b || dep.starts_with(&format!("{b}-"))),
                "крейт {c} зависит от {dep}, а от него зависит sim-core"
            );
        }
    }
}

/// **Владеть `wgpu` разрешено ровно двум крейтам** — `render` и `gpu`.
#[test]
fn only_the_render_layer_owns_wgpu() {
    let ms = all_manifests();
    for (name, m) in &ms {
        for (dep, _) in &m.external {
            if dep == "wgpu" || dep == "bevy" || dep == "winit" {
                assert!(
                    name == "gpu" || name == "render",
                    "{name} владеет {dep}, а это разрешено только render и gpu"
                );
            }
        }
    }
}

/// **Версии пинятся точно** (`CLAUDE.md`). Не пин — это молчаливое
/// обновление, которое сдвинет golden-хеши между машинами.
#[test]
fn every_external_dependency_is_pinned_exactly() {
    let ms = all_manifests();
    for (name, m) in &ms {
        for (dep, spec) in &m.external {
            // Версия может быть строкой или таблицей с полем version.
            let v = if let Some(i) = spec.find("version") {
                spec[i..].split('"').nth(1).unwrap_or("")
            } else {
                spec.trim_matches(|c| c == '"' || c == ' ')
            };
            assert!(
                v.starts_with('='),
                "{name}: {dep} = {spec} — версия не запинена точно"
            );
        }
    }
}

/// Исходники крейта **без тестов и без `src/bin`**.
///
/// Тесты режутся по первому `#[cfg(test)]` до конца файла: так они
/// в этом проекте и написаны, а тащить разбор Rust ради проверки
/// нельзя. Стенды в `src/bin` печатают миллисекунды и к состоянию
/// симуляции отношения не имеют.
fn production_sources(dir: &Path) -> Vec<(PathBuf, String)> {
    sources(dir, true)
}

/// Исходники крейта **вместе** с `src/bin`.
///
/// У sim-крейтов `src/bin` — стенды, и пропускать их правильно.
/// У рендера в `src/bin` лежит весь код окна: `battle`, `view`,
/// `parade`. Первая редакция проверки границы слоёв пользовалась
/// `production_sources` и потому смотрела куда угодно, кроме предмета
/// проверки, — подложенная в `battle.rs` запись в состояние симуляции
/// прошла мимо неё (ADR-184).
fn all_sources(dir: &Path) -> Vec<(PathBuf, String)> {
    sources(dir, false)
}

fn sources(dir: &Path, skip_bin: bool) -> Vec<(PathBuf, String)> {
    fn walk(p: &Path, out: &mut Vec<PathBuf>, skip_bin: bool) {
        let Ok(rd) = std::fs::read_dir(p) else { return };
        let mut items: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
        items.sort();
        for i in items {
            if i.is_dir() {
                if skip_bin && i.file_name().map(|n| n == "bin").unwrap_or(false) {
                    continue;
                }
                walk(&i, out, skip_bin);
            } else if i.extension().map(|e| e == "rs").unwrap_or(false) {
                out.push(i);
            }
        }
    }
    let mut files = Vec::new();
    walk(&dir.join("src"), &mut files, skip_bin);
    files
        .into_iter()
        .map(|f| {
            let text = std::fs::read_to_string(&f).unwrap_or_default();
            let prod = without_test_code(&text);
            (f, prod)
        })
        .collect()
}

/// Исходник без всего, что помечено `#[cfg(test)]`.
///
/// **Не обрезка по первому вхождению.** В `sim-core/src/lib.rs`
/// помощник `#[cfg(test)] fn test_tuning()` стоит на 188-й строке,
/// а `mod tests` — на 1904-й. Обрезка по первому прятала от всех
/// линтов 1715 строк боевого кода: шестнадцать фаз тика, движение,
/// бой, стрельбу и сам хеш. Исключения ADR-162 были выписаны для
/// строк, которых проверка никогда не видела (ADR-180).
///
/// Пропущенные строки заменяются пустыми, а не выбрасываются: линты
/// печатают номер строки, и сдвиг нумерации сделал бы их выдачу ложью.
fn without_test_code(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut skipping = false;
    let mut depth = 0i32;
    let mut opened = false;
    for line in text.lines() {
        if !skipping {
            if line.trim_start().starts_with("#[cfg(test)]") {
                skipping = true;
                depth = 0;
                opened = false;
                out.push('\n');
                continue;
            }
            out.push_str(line);
            out.push('\n');
            continue;
        }
        depth += braces(line);
        if line.contains('{') {
            opened = true;
        }
        out.push('\n');
        if opened && depth <= 0 {
            skipping = false;
        }
    }
    out
}

/// Перевес открывающих фигурных скобок над закрывающими.
///
/// Литералы и хвостовой комментарий пропускаются: `'{'` внутри строки
/// увёл бы счётчик и съел остаток файла — то есть спрятал бы код
/// ровно так же, как обрезка по первому `#[cfg(test)]`.
fn braces(line: &str) -> i32 {
    let b: Vec<char> = line.chars().collect();
    let mut n = 0i32;
    let mut i = 0usize;
    while i < b.len() {
        match b[i] {
            '/' if i + 1 < b.len() && b[i + 1] == '/' => break,
            '"' => {
                i += 1;
                while i < b.len() && b[i] != '"' {
                    i += if b[i] == '\\' { 2 } else { 1 };
                }
            }
            // Символьный литерал, но не время жизни `'a`.
            '\'' if i + 2 < b.len() && b[i + 2] == '\'' => i += 2,
            '\'' if i + 3 < b.len() && b[i + 1] == '\\' && b[i + 3] == '\'' => i += 3,
            '{' => n += 1,
            '}' => n -= 1,
            _ => {}
        }
        i += 1;
    }
    n
}

/// Строка кода, а не комментария.
///
/// Ведущей `*` здесь отсеивались строки продолжения блочного
/// комментария. Блочных комментариев в проекте **нет ни одного**,
/// а строк боевого кода, начинающихся с разыменования (`*slot = ...`,
/// `*blocked_moves += 1`), — 39, и все они были невидимы линту.
/// Правило защищало то, чего нет, ценой того, что есть (ADR-180).
fn is_code(line: &str) -> bool {
    let t = line.trim_start();
    !(t.starts_with("//") || t.starts_with("#!["))
}

/// **Линт видит боевой код целиком.**
///
/// Сторож на сам сторож. Три проверки ниже — на плавающее, на
/// хеш-контейнеры и на часы — стоят ровно столько, сколько исходника
/// они видят, и полтора года видели 62% (ADR-180).
#[test]
fn the_lint_sees_the_whole_production_file() {
    // Синтетика: помощник, помеченный `#[cfg(test)]`, не должен
    // прятать то, что идёт за ним.
    let src = concat!(
        "fn a() {}\n",
        "#[cfg(test)]\n",
        "fn helper() { let s = \"{\"; }\n",
        "fn b() { let _: f64 = 0.0; }\n",
        "#[cfg(test)]\n",
        "mod tests {\n    fn t() {}\n}\n",
    );
    let kept = without_test_code(src);
    assert!(kept.contains("fn a()"), "потерян код до помощника");
    assert!(kept.contains("f64"), "боевой код за помощником пропал");
    assert!(!kept.contains("helper"), "тестовый помощник остался");
    assert!(!kept.contains("fn t()"), "тестовый модуль остался");
    assert_eq!(
        kept.lines().count(),
        src.lines().count(),
        "нумерация строк поехала — линт будет печатать чужие номера"
    );

    // И на настоящем файле: самая дальняя боевая фаза видна.
    // `sim-core/src/lib.rs` — тот файл, ради которого всё это.
    let f = root().join("crates/sim-core/src/lib.rs");
    let prod = without_test_code(&std::fs::read_to_string(&f).expect("sim-core/src/lib.rs"));
    for marker in ["fn step_shots", "fn phase_hash", "fn apply_deltas"] {
        assert!(prod.contains(marker), "линт не видит {marker}");
    }
    assert!(
        !prod.contains("fn test_tuning"),
        "тестовый помощник просочился в боевой текст"
    );
}

/// Часть строки слева от присваивания, если оно в ней есть.
///
/// Нужна, чтобы отличить `units.x[i] = 5` от `let n = units.live()`:
/// первое — запись в состояние симуляции, второе — чтение. Различает
/// их только то, с какой стороны от знака стоит путь.
fn assigned_path(line: &str) -> Option<&str> {
    let b = line.as_bytes();
    let mut i = 0usize;
    while i < b.len() {
        match b[i] {
            b'/' if i + 1 < b.len() && b[i + 1] == b'/' => return None,
            b'"' => {
                i += 1;
                while i < b.len() && b[i] != b'"' {
                    i += if b[i] == b'\\' { 2 } else { 1 };
                }
            }
            b'=' => {
                // Не сравнение и не стрелка.
                let prev = if i > 0 { b[i - 1] } else { b' ' };
                let next = if i + 1 < b.len() { b[i + 1] } else { b' ' };
                if next == b'=' || matches!(prev, b'=' | b'!' | b'<' | b'>') {
                    i += 1;
                } else {
                    return Some(&line[..i]);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// **Рендер читает состояние симуляции и никогда не пишет в него.**
///
/// `CLAUDE.md` называет нарушение этой границы откатом, а проверки
/// на неё не было: соседний тест сторожит только направление
/// зависимостей в `Cargo.toml`. Поля `Units` и `Shots` публичны,
/// `World::ground_mut` тоже — запись компилируется в одну строку
/// и отличается от чтения ровно тем, с какой стороны от `=` стоит
/// путь (ADR-184).
///
/// Тикать симуляцию рендеру **можно**: окно владеет ею и двигает
/// её вперёд целиком, а не правит по полю. Запрещено именно
/// присваивание в её состояние и постановка правок мира в обход
/// симуляции.
#[test]
fn the_render_layer_never_writes_into_the_simulation() {
    // Прямые ходы в мир мимо симуляции — запрещены целиком.
    const BANNED: [&str; 3] = ["ground_mut", "queue_dig", "queue_edit"];
    // Пути состояния симуляции: присваивание в них — нарушение.
    const STATE: [&str; 4] = [".units.", ".shots.", ".bodies.", ".world."];

    let mut bad: Vec<String> = Vec::new();
    // **Вместе с `src/bin`**: там и лежит весь код окна. Смотри
    // `all_sources` — первая редакция этой проверки была слепа ровно
    // к тому, что проверяет.
    let files = all_sources(&root().join("crates").join("render"));
    assert!(
        files.iter().any(|(f, _)| f.ends_with("battle.rs")),
        "проверка не видит окно боя — она смотрит не туда"
    );
    for (f, text) in files {
        for (n, line) in text.lines().enumerate() {
            if !is_code(line) {
                continue;
            }
            if let Some(call) = BANNED.iter().find(|b| line.contains(**b)) {
                bad.push(format!(
                    "{}:{}: {call} — {}",
                    f.display(),
                    n + 1,
                    line.trim()
                ));
                continue;
            }
            let Some(lhs) = assigned_path(line) else {
                continue;
            };
            if let Some(path) = STATE.iter().find(|p| lhs.contains(**p)) {
                bad.push(format!(
                    "{}:{}: присваивание в {path} — {}",
                    f.display(),
                    n + 1,
                    line.trim()
                ));
            }
        }
    }
    assert!(
        bad.is_empty(),
        "рендер пишет в состояние симуляции:\n{}",
        bad.join("\n")
    );

    // Сторож на сам сторож: чтение не должно ловиться, запись должна.
    assert!(assigned_path("let n = sim.r.world.units.live().len();").is_some());
    assert!(
        !assigned_path("let n = sim.r.world.units.live().len();")
            .expect("присваивание есть")
            .contains(".units."),
        "чтение принято за запись"
    );
    assert!(
        assigned_path("sim.r.world.units.x[i] = 5;")
            .expect("присваивание есть")
            .contains(".units."),
        "запись в состояние не распознана"
    );
    assert!(
        assigned_path("if a.units.x[i] == b { }").is_none(),
        "сравнение принято за присваивание"
    );
}

/// **Ни `f32`, ни `f64` в симуляции** (`CLAUDE.md`). Исключения
/// перечислены поимённо и объяснены: молчаливое исключение — это
/// отсутствие правила.
#[test]
fn the_simulation_has_no_floating_point() {
    // `Fx::to_f64_lossy` и `Debug` для `Fx` — единственный мост наружу:
    // ими пользуются стенды и рендер, в арифметику они не входят.
    let allow: &[(&str, &str)] = &[
        ("fx", "to_f64_lossy"),
        ("fx", "self.0 as f64 / ONE_RAW as f64"),
        ("fx", "self.to_f64_lossy()"),
    ];
    let ms = all_manifests();
    let mut bad: Vec<String> = Vec::new();
    for c in sim_closure(&ms) {
        for (f, text) in production_sources(&root().join("crates").join(&c)) {
            for (n, line) in text.lines().enumerate() {
                if !is_code(line) || !(line.contains("f32") || line.contains("f64")) {
                    continue;
                }
                if allow
                    .iter()
                    .any(|(cr, frag)| *cr == c && line.contains(frag))
                {
                    continue;
                }
                bad.push(format!("{}:{}: {}", f.display(), n + 1, line.trim()));
            }
        }
    }
    assert!(bad.is_empty(), "плавающее в симуляции:\n{}", bad.join("\n"));
}

/// **Ни `HashMap`, ни `HashSet` в итерируемых путях** (`CLAUDE.md`):
/// порядок обхода обязан быть стабильным.
#[test]
fn the_simulation_has_no_hash_containers() {
    let ms = all_manifests();
    let mut bad: Vec<String> = Vec::new();
    for c in sim_closure(&ms) {
        for (f, text) in production_sources(&root().join("crates").join(&c)) {
            for (n, line) in text.lines().enumerate() {
                if is_code(line) && (line.contains("HashMap") || line.contains("HashSet")) {
                    bad.push(format!("{}:{}: {}", f.display(), n + 1, line.trim()));
                }
            }
        }
    }
    assert!(
        bad.is_empty(),
        "хеш-контейнер в симуляции:\n{}",
        bad.join("\n")
    );
}

/// **Симуляция не смотрит на часы.**
///
/// `CLAUDE.md` запрещает `std::time` в `sim-core` и всём, от чего он
/// зависит, вместе с `rand::thread_rng`, вводом-выводом и глобальным
/// состоянием. Запрет был записан, а проверки на него не было —
/// при том, что рядом стоят такие же проверки на плавающее и на
/// хеш-контейнеры (ADR-162).
///
/// Исключение одно и названо поимённо: **потиковый таймер фаз**
/// в `sim-core`. Он пишет в `phase_ns`, который **не входит в хеш**,
/// и стоит меньше разброса замера. Без него не найти фазу, работающую
/// вхолостую, — а это тот самый класс ошибок, на котором стоят
/// ADR-116, ADR-157 и ADR-159.
///
/// Исключение перечислено строками, а не крейтом: разрешено ровно то,
/// что уже есть, и ни строкой больше.
#[test]
fn the_simulation_never_reads_the_clock() {
    // (крейт, фрагмент строки) — ровно таймер фаз, и ничего кроме.
    let allow: &[(&str, &str)] = &[
        ("sim-core", "use std::time::Instant;"),
        ("sim-core", "let t0 = Instant::now();"),
        (
            "sim-core",
            "self.phase_ns[pi] += t0.elapsed().as_nanos() as u64;",
        ),
    ];
    let ms = all_manifests();
    let mut bad: Vec<String> = Vec::new();
    for c in sim_closure(&ms) {
        for (f, text) in production_sources(&root().join("crates").join(&c)) {
            for (n, line) in text.lines().enumerate() {
                let hit = line.contains("std::time")
                    || line.contains("Instant::now")
                    || line.contains("SystemTime")
                    || line.contains("thread_rng");
                if !is_code(line) || !hit {
                    continue;
                }
                if allow
                    .iter()
                    .any(|(cr, frag)| *cr == c && line.contains(frag))
                {
                    continue;
                }
                bad.push(format!("{}:{}: {}", f.display(), n + 1, line.trim()));
            }
        }
    }
    assert!(bad.is_empty(), "часы в симуляции:\n{}", bad.join("\n"));
}

/// Состав рабочей области совпадает с объявленным в
/// `docs/02-architecture.md`. Проверка односторонняя: лишний крейт —
/// ошибка, отсутствующий — нет, потому что `render` ещё не написан.
#[test]
fn every_crate_is_one_the_architecture_declares() {
    let declared = [
        "fx", "voxel", "worldgen", "nav", "modding", "phys", "sim-core", "sim-cli", "meshing",
        "gpu", "render", "demo", "harness",
    ];
    for name in all_manifests().keys() {
        assert!(
            declared.contains(&name.as_str()),
            "крейт {name} не объявлен в docs/02-architecture.md"
        );
    }
}
