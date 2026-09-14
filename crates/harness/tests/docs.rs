//! Связность документов — **проверкой, а не перечитыванием**.
//!
//! Три правила `CLAUDE.md` держались на внимательности:
//!
//! - каждое решение имеет строку в реестре `docs/12-decisions.md`
//!   **и** полный текст в `docs/analysis/10-decisions-full.md`;
//! - ссылок на несуществующие `ADR-NNN` в документах нет;
//! - когда вопрос закрыт, пометка удаляется, и **все входящие ссылки
//!   на этот `Q-NN` чинятся тем же коммитом** — иначе в документах
//!   остаётся ссылка на несуществующий вопрос, которая читается как
//!   открытый блокер.
//!
//! Проверял их человек, перечитывая оба файла целиком. Одна такая
//! ссылка и висела — `Q-64` в строке реестра ADR-145 (ADR-186).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(rel: &str) -> String {
    let p = root().join(rel);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// Все файлы канона — `docs/*.md`, без подкаталога анализа.
fn canon_files() -> Vec<(String, String)> {
    let dir = root().join("docs");
    let mut out = Vec::new();
    let rd = std::fs::read_dir(&dir).expect("docs/");
    let mut items: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
    items.sort();
    for p in items {
        if p.extension().map(|e| e == "md").unwrap_or(false) {
            let name = p.file_name().expect("имя").to_string_lossy().into_owned();
            out.push((name, std::fs::read_to_string(&p).unwrap_or_default()));
        }
    }
    assert!(out.len() > 5, "канон не нашёлся: {} файлов", out.len());
    out
}

/// Числа после заданного префикса, по порядку появления.
fn numbers_after(text: &str, prefix: &str) -> Vec<u32> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find(prefix) {
        rest = &rest[i + prefix.len()..];
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if !digits.is_empty() {
            if let Ok(n) = digits.parse() {
                out.push(n);
            }
        }
    }
    out
}

/// Номера ADR из реестра: строка таблицы вида `| 145 | ... |`.
fn registry() -> Vec<u32> {
    read("docs/12-decisions.md")
        .lines()
        .filter_map(|l| {
            let t = l.trim();
            let cell = t.strip_prefix('|')?.split('|').next()?.trim();
            if cell.len() == 3 && cell.chars().all(|c| c.is_ascii_digit()) {
                cell.parse().ok()
            } else {
                None
            }
        })
        .collect()
}

/// Номера ADR из полного текста: заголовок `## ADR-145 — ...`.
fn full_text() -> Vec<u32> {
    read("docs/analysis/10-decisions-full.md")
        .lines()
        .filter_map(|l| l.strip_prefix("## ADR-"))
        .filter_map(|r| {
            let d: String = r.chars().take_while(|c| c.is_ascii_digit()).collect();
            d.parse().ok()
        })
        .collect()
}

/// **Реестр и полный текст — один список.**
///
/// Решение, у которого есть строка в реестре и нет текста, читается
/// как принятое и необоснованное; обратное — как принятое втайне.
#[test]
fn every_decision_is_in_both_files() {
    let (reg, full) = (registry(), full_text());
    assert!(reg.len() > 100, "реестр не разобрался: {} строк", reg.len());

    let dup = |v: &[u32]| -> Vec<u32> {
        let mut seen = BTreeSet::new();
        v.iter().filter(|n| !seen.insert(**n)).copied().collect()
    };
    assert!(dup(&reg).is_empty(), "дубли в реестре: {:?}", dup(&reg));
    assert!(dup(&full).is_empty(), "дубли в тексте: {:?}", dup(&full));

    let (rs, fs): (BTreeSet<u32>, BTreeSet<u32>) = (
        reg.iter().copied().collect(),
        full.iter().copied().collect(),
    );
    let only_reg: Vec<u32> = rs.difference(&fs).copied().collect();
    let only_full: Vec<u32> = fs.difference(&rs).copied().collect();
    assert!(
        only_reg.is_empty() && only_full.is_empty(),
        "реестр и полный текст разошлись:\nтолько в реестре {only_reg:?}\nтолько в тексте {only_full:?}"
    );
}

/// **Ссылок на несуществующие решения нет.**
///
/// `ADR-NNN` в тексте — обещание, что такое решение принято. Ссылка
/// на непринятое читается как принятое.
#[test]
fn every_referenced_decision_exists() {
    let known: BTreeSet<u32> = registry().into_iter().collect();
    let mut bad: Vec<String> = Vec::new();
    for (name, text) in canon_files() {
        for n in numbers_after(&text, "ADR-") {
            if !known.contains(&n) {
                bad.push(format!("docs/{name}: ADR-{n:03}"));
            }
        }
    }
    for n in numbers_after(&read("docs/analysis/10-decisions-full.md"), "ADR-") {
        if !known.contains(&n) {
            bad.push(format!("10-decisions-full.md: ADR-{n:03}"));
        }
    }
    // **Код и данные проверяются наравне с документами.** Первая
    // редакция смотрела только в `docs/`, и четыре решения — 219, 221,
    // 225, 226 — жили одними комментариями в коде и сообщениями
    // коммитов: реестр о них не знал, и никто этого не заметил, пока
    // не пересчитали руками. Ссылка в коде — то же обещание, что
    // и в каноне.
    for (path, text) in sources_and_data() {
        for n in numbers_after(&text, "ADR-") {
            if !known.contains(&n) {
                bad.push(format!("{path}: ADR-{n:03}"));
            }
        }
    }
    bad.sort();
    bad.dedup();
    assert!(
        bad.is_empty(),
        "ссылки на несуществующие решения:\n{}",
        bad.join("\n")
    );
}

/// Все исходники и данные: `crates/**/*.rs` и `data/**/*.ron`.
///
/// Каталоги сборки и чужие рабочие деревья под `.claude/` сюда
/// не попадают: обход начинается с `crates/` и `data/`.
fn sources_and_data() -> Vec<(String, String)> {
    fn walk(dir: &std::path::Path, out: &mut Vec<(String, String)>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        let mut items: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
        items.sort();
        for p in items {
            if p.is_dir() {
                if p.file_name().map(|n| n == "target").unwrap_or(false) {
                    continue;
                }
                walk(&p, out);
            } else if p
                .extension()
                .map(|e| e == "rs" || e == "ron")
                .unwrap_or(false)
            {
                let rel = p
                    .strip_prefix(root())
                    .unwrap_or(&p)
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push((rel, std::fs::read_to_string(&p).unwrap_or_default()));
            }
        }
    }
    let mut out = Vec::new();
    walk(&root().join("crates"), &mut out);
    walk(&root().join("data"), &mut out);
    assert!(out.len() > 20, "исходники не нашлись: {} файлов", out.len());
    out
}

/// **В каноне нет ссылок на закрытые вопросы.**
///
/// `CLAUDE.md`: пометка удаляется, текст становится утвердительным,
/// обоснование уходит в ADR — и все входящие ссылки чинятся тем же
/// коммитом. Оставшийся `Q-NN` читается как открытый блокер, которого
/// нет.
///
/// Анализ не проверяется: он ненормативен, и упоминание закрытого
/// вопроса там — история, а не блокер.
#[test]
fn the_canon_never_points_at_a_closed_question() {
    let files = canon_files();
    // Открытые вопросы: пометки живут только в `docs/`.
    let mut open: BTreeSet<u32> = BTreeSet::new();
    for (_, text) in &files {
        for n in numbers_after(text, "[QUESTION Q-") {
            open.insert(n);
        }
    }
    assert!(
        !open.is_empty(),
        "ни одной пометки не нашлось — разбор сломан"
    );

    let mut bad: Vec<String> = Vec::new();
    for (name, text) in &files {
        for n in numbers_after(text, "Q-") {
            if !open.contains(&n) {
                bad.push(format!("docs/{name}: Q-{n}"));
            }
        }
    }
    bad.sort();
    bad.dedup();
    assert!(
        bad.is_empty(),
        "канон ссылается на закрытые вопросы — они читаются как открытые блокеры:\n{}",
        bad.join("\n")
    );
}
