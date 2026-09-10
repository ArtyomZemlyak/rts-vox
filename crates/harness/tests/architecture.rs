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
        let Some((k, v)) = t.split_once('=') else { continue };
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
    Manifest { name, local, external }
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
                !banned.iter().any(|b| dep == b || dep.starts_with(&format!("{b}-"))),
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
    fn walk(p: &Path, out: &mut Vec<PathBuf>) {
        let Ok(rd) = std::fs::read_dir(p) else { return };
        let mut items: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
        items.sort();
        for i in items {
            if i.is_dir() {
                if i.file_name().map(|n| n == "bin").unwrap_or(false) {
                    continue;
                }
                walk(&i, out);
            } else if i.extension().map(|e| e == "rs").unwrap_or(false) {
                out.push(i);
            }
        }
    }
    let mut files = Vec::new();
    walk(&dir.join("src"), &mut files);
    files
        .into_iter()
        .map(|f| {
            let text = std::fs::read_to_string(&f).unwrap_or_default();
            let cut = text.find("#[cfg(test)]").unwrap_or(text.len());
            (f, text[..cut].to_string())
        })
        .collect()
}

/// Строка кода, а не комментария и не строкового литерала документации.
fn is_code(line: &str) -> bool {
    let t = line.trim_start();
    !(t.starts_with("//") || t.starts_with("*") || t.starts_with("#!["))
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
    assert!(bad.is_empty(), "хеш-контейнер в симуляции:\n{}", bad.join("\n"));
}

/// Состав рабочей области совпадает с объявленным в
/// `docs/02-architecture.md`. Проверка односторонняя: лишний крейт —
/// ошибка, отсутствующий — нет, потому что `render` ещё не написан.
#[test]
fn every_crate_is_one_the_architecture_declares() {
    let declared = [
        "fx", "voxel", "worldgen", "nav", "modding", "sim-core", "sim-cli", "meshing", "gpu",
        "render", "demo", "harness",
    ];
    for name in all_manifests().keys() {
        assert!(
            declared.contains(&name.as_str()),
            "крейт {name} не объявлен в docs/02-architecture.md"
        );
    }
}
