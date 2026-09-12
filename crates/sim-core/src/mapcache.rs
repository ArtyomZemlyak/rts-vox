//! Кэш карты: битовый объём занятости и слой навигации нетронутого
//! мира — **в буфер и обратно** (ADR-228).
//!
//! # Зачем
//!
//! Процедурный слой — чистая функция от сида, но построить его
//! на километре стоит минуты: битовый объём — обход генератора по всем
//! колонкам, слой навигации — обход объёма по всем клеткам. Ни то,
//! ни другое не меняется от запуска к запуску, и строить карту каждый
//! раз незачем: один раз построенная, она читается с диска за секунды.
//!
//! # Что здесь не лежит
//!
//! Слой изменений, юниты, очередь правок — это **партия**, а не карта.
//! Формат сохранения партии — отдельное решение, и кэш ему не замена
//! и не часть: партия ложится **поверх** мира, собранного из сида,
//! и ей всё равно, пришёл этот мир из генератора или из кэша.
//!
//! Файлов здесь нет: `sim-core` не знает про диск (`CLAUDE.md`).
//! Байты читает и пишет тот, кто запускает.
//!
//! # Ключ и сверка
//!
//! Ключ — все параметры генератора, сид, размеры и числа навигации.
//! Ключ **не доказывает** совпадения: поле, добавленное в `Params`
//! и забытое здесь, дало бы тот же ключ и чужую карту. Поэтому при
//! чтении объём **сверяется с генератором выборкой** — четыре тысячи
//! вокселей по явному сиду, миллисекунда работы — и чужая карта
//! отвергается, а не показывается.
//!
//! # Формат
//!
//! От младшего байта, поля явных размеров, без `serde`: раскладка
//! задаётся этим файлом, а не тем, как сегодня объявлены структуры.
//!
//! ```text
//! 0   8   "RTSVOXMC"
//! 8   4   версия
//! 12  8   ключ
//! 20  12  nx, ny, nz
//! 32  8   слов объёма
//! 40  …   объём занятости, u64 подряд
//!     …   навигация: w, h, height, step, max_step (u32), overflow (u64),
//!         floor u16[w·h], levels u16[w·h·8], n_levels u8[w·h],
//!         top u16[w·h], stand u8[w·h]
//! ```
//!
//! Версия растёт при любом изменении раскладки; незнакомая версия
//! отвергается с внятным сообщением.

use crate::ground::Ground;
use crate::Tuning;
use fx::rng::Rng;
use nav::layer::{NavLayer, NavRaw, MAX_LEVELS};
use voxel::store::AIR;
use voxel::Generator;

/// Подпись файла.
pub const MAGIC: [u8; 8] = *b"RTSVOXMC";

/// Версия раскладки. Меняется вместе с любым полем формата.
pub const VERSION: u32 = 1;

/// Сколько вокселей сверяется с генератором при чтении.
pub const CHECK_SAMPLES: u32 = 4096;

/// Почему кэш не прочитан.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheError {
    /// Не наш файл.
    NotACache,
    /// Наш, но другой версии.
    Version { found: u32, want: u32 },
    /// Другая карта: не тот сид, размер или параметры.
    Key { found: u64, want: u64 },
    /// Оборвался или испорчен.
    Truncated,
    /// Ключ сошёлся, а содержимое с генератором — нет.
    Mismatch,
}

impl std::fmt::Display for CacheError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CacheError::NotACache => write!(f, "это не кэш карты rts-vox: подпись файла не та"),
            CacheError::Version { found, want } => write!(
                f,
                "кэш карты версии {found}, а читать умеем {want}: формат менялся"
            ),
            CacheError::Key { found, want } => write!(
                f,
                "кэш другой карты: ключ {found:016x}, а нужен {want:016x}"
            ),
            CacheError::Truncated => write!(f, "кэш карты обрезан или испорчен"),
            CacheError::Mismatch => write!(
                f,
                "кэш карты не совпал с генератором: содержимое не от этого мира"
            ),
        }
    }
}

impl std::error::Error for CacheError {}

fn fnv(h: &mut u64, v: u64) {
    for b in v.to_le_bytes() {
        *h ^= u64::from(b);
        *h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
}

/// Ключ карты: параметры генератора, сид и числа навигации.
///
/// Перечислены руками, каждое поле своей строкой: набор полей
/// у `Params` растёт, и новое поле обязано попасть сюда. Забытое
/// ловит выборочная сверка при чтении, но ловит как порчу, а не
/// как другую карту.
pub fn key(p: &worldgen::Params, seed: u64, t: &Tuning) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    fnv(&mut h, u64::from(VERSION));
    for v in [
        p.nx,
        p.ny,
        p.nz,
        p.base_height,
        p.relief,
        p.feature,
        p.detail,
        p.soil,
        p.lava_depth,
        p.deep_at,
        p.vein_threshold,
        p.terrace,
        p.terrace_lo,
        p.terrace_hi,
        p.bed,
        p.plains,
        p.plains_period,
        p.plains_floor,
    ] {
        fnv(&mut h, u64::from(v));
    }
    fnv(&mut h, seed);
    for v in [t.unit_height, t.nav_cell, t.max_step] {
        fnv(&mut h, u64::from(v));
    }
    h
}

fn put_u16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn put_u64(out: &mut Vec<u8>, v: u64) {
    out.extend_from_slice(&v.to_le_bytes());
}

/// Записать карту в буфер.
pub fn encode(world: &voxel::World, nav: &NavLayer, key: u64) -> Vec<u8> {
    let bits = world.bits();
    let r = nav.raw();
    let cells = r.floor.len();
    let mut out = Vec::with_capacity(72 + bits.len() * 8 + cells * (2 + 2 * MAX_LEVELS + 1 + 2 + 1));
    out.extend_from_slice(&MAGIC);
    put_u32(&mut out, VERSION);
    put_u64(&mut out, key);
    put_u32(&mut out, world.nx);
    put_u32(&mut out, world.ny);
    put_u32(&mut out, world.nz);
    put_u64(&mut out, bits.len() as u64);
    for &w in bits {
        out.extend_from_slice(&w.to_le_bytes());
    }
    put_u32(&mut out, r.w);
    put_u32(&mut out, r.h);
    put_u32(&mut out, r.height);
    put_u32(&mut out, r.step);
    put_u32(&mut out, r.max_step);
    put_u64(&mut out, r.overflow);
    for &v in &r.floor {
        put_u16(&mut out, v);
    }
    for &v in &r.levels {
        put_u16(&mut out, v);
    }
    out.extend_from_slice(&r.n_levels);
    for &v in &r.top {
        put_u16(&mut out, v);
    }
    out.extend(r.stand.iter().map(|&b| u8::from(b)));
    out
}

/// Чтение с проверкой длины на каждом поле. Недочитанное — не паника,
/// а [`CacheError::Truncated`].
struct Cursor<'a> {
    b: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], CacheError> {
        let end = self.at.checked_add(n).ok_or(CacheError::Truncated)?;
        if end > self.b.len() {
            return Err(CacheError::Truncated);
        }
        let s = &self.b[self.at..end];
        self.at = end;
        Ok(s)
    }

    fn u32(&mut self) -> Result<u32, CacheError> {
        let s = self.take(4)?;
        Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    }

    fn u64(&mut self) -> Result<u64, CacheError> {
        let s = self.take(8)?;
        let mut a = [0u8; 8];
        a.copy_from_slice(s);
        Ok(u64::from_le_bytes(a))
    }

    fn u8s(&mut self, n: usize) -> Result<Vec<u8>, CacheError> {
        Ok(self.take(n)?.to_vec())
    }

    fn u16s(&mut self, n: usize) -> Result<Vec<u16>, CacheError> {
        let s = self.take(n.checked_mul(2).ok_or(CacheError::Truncated)?)?;
        Ok(s.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect())
    }

    fn u64s(&mut self, n: usize) -> Result<Vec<u64>, CacheError> {
        let s = self.take(n.checked_mul(8).ok_or(CacheError::Truncated)?)?;
        Ok(s
            .chunks_exact(8)
            .map(|c| {
                let mut a = [0u8; 8];
                a.copy_from_slice(c);
                u64::from_le_bytes(a)
            })
            .collect())
    }
}

/// Прочитать карту из буфера: мир поверх готового объёма и слой
/// навигации. Ключ обязан сойтись, объём — пройти выборочную сверку
/// с генератором.
pub fn decode(
    bytes: &[u8],
    p: &worldgen::Params,
    seed: u64,
    t: &Tuning,
    max_edits: usize,
) -> Result<(voxel::World, NavLayer), CacheError> {
    if bytes.len() < MAGIC.len() || bytes[..MAGIC.len()] != MAGIC {
        return Err(CacheError::NotACache);
    }
    let mut s = Cursor { b: &bytes[MAGIC.len()..], at: 0 };
    let v = s.u32()?;
    if v != VERSION {
        return Err(CacheError::Version { found: v, want: VERSION });
    }
    let want = key(p, seed, t);
    let found = s.u64()?;
    if found != want {
        return Err(CacheError::Key { found, want });
    }
    let (nx, ny, nz) = (s.u32()?, s.u32()?, s.u32()?);
    if (nx, ny, nz) != (p.nx, p.ny, p.nz) {
        return Err(CacheError::Mismatch);
    }
    let words = s.u64()? as usize;
    let wx = nx.div_ceil(64) as usize;
    if words != wx * ny as usize * nz as usize {
        return Err(CacheError::Mismatch);
    }
    let bits = s.u64s(words)?;

    // Выборочная сверка с генератором: ключ говорит, что карта та же,
    // а это — что она та же на самом деле.
    let gen = worldgen::Terrain { params: *p, seed };
    if nx > 0 && ny > 0 && nz > 0 {
        let mut rng = Rng::new(seed, 0xCA5E);
        for _ in 0..CHECK_SAMPLES {
            let (x, y, z) = (rng.below(nx), rng.below(ny), rng.below(nz));
            let w = (y as usize * nz as usize + z as usize) * wx + (x >> 6) as usize;
            let bit = (bits[w] >> (x & 63)) & 1 != 0;
            if bit != (gen.material(x, y, z) != AIR) {
                return Err(CacheError::Mismatch);
            }
        }
    }

    let (w, h, height, step, max_step) = (s.u32()?, s.u32()?, s.u32()?, s.u32()?, s.u32()?);
    let overflow = s.u64()?;
    if step == 0
        || (height, step, max_step) != (t.unit_height, t.nav_cell, t.max_step)
        || (w, h) != (nx / step, nz / step)
    {
        return Err(CacheError::Mismatch);
    }
    let cells = w as usize * h as usize;
    let raw = NavRaw {
        w,
        h,
        height,
        step,
        max_step,
        overflow,
        floor: s.u16s(cells)?,
        levels: s.u16s(cells * MAX_LEVELS)?,
        n_levels: s.u8s(cells)?,
        top: s.u16s(cells)?,
        stand: s.u8s(cells)?.into_iter().map(|b| b != 0).collect(),
    };
    let nav = NavLayer::from_raw(raw).ok_or(CacheError::Mismatch)?;
    let world = voxel::World::from_bits(nx, ny, nz, max_edits, Box::new(gen), bits)
        .ok_or(CacheError::Mismatch)?;
    Ok((world, nav))
}

/// Земля из кэша: мир и навигация — из буфера, связность и каскад —
/// ленивые, как обычно.
pub fn from_cache(
    bytes: &[u8],
    p: &worldgen::Params,
    seed: u64,
    t: &Tuning,
    max_edits: usize,
) -> Result<Ground, CacheError> {
    let (world, nav) = decode(bytes, p, seed, t, max_edits)?;
    Ok(Ground::with_nav(world, nav, t))
}

/// Кэш **нетронутой** земли. Слой изменений в него не идёт: это
/// партия, а не карта, и вызывающий обязан снимать кэш до первой
/// правки — иначе в кэше окажется чужая яма.
pub fn to_cache(g: &Ground, key: u64) -> Vec<u8> {
    encode(&g.world, &g.nav, key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{test_tuning, test_weapons, Reference, REF_MAX_EDITS};

    fn params() -> worldgen::Params {
        worldgen::Params::sized(96, 48, 80)
    }

    fn built(seed: u64) -> (voxel::World, NavLayer) {
        let p = params();
        let t = test_tuning();
        let w = voxel::World::new(p.nx, p.ny, p.nz, REF_MAX_EDITS, Box::new(worldgen::Terrain { params: p, seed }));
        let nav = NavLayer::new(&w, t.unit_height, t.nav_cell, t.max_step);
        (w, nav)
    }

    /// **Загруженная карта равна сгенерированной** — до бита объёма
    /// и до клетки навигации.
    #[test]
    fn a_loaded_map_equals_the_generated_one() {
        let (p, t, seed) = (params(), test_tuning(), 11u64);
        let (w, nav) = built(seed);
        let k = key(&p, seed, &t);
        let bytes = encode(&w, &nav, k);
        let (w2, nav2) = decode(&bytes, &p, seed, &t, REF_MAX_EDITS).expect("кэш читается");
        assert_eq!(w2.bits(), w.bits(), "объём разошёлся");
        assert_eq!(nav2.raw(), nav.raw(), "навигация разошлась");
        assert!(w.bits().iter().any(|&v| v != 0), "мир пуст — сверять нечего");
    }

    /// **Партия на карте из кэша — та же партия.** Хеш тика через
    /// пятьдесят тиков совпадает с партией на карте из генератора;
    /// иначе кэш — это не кэш, а другой мир.
    #[test]
    fn a_game_on_a_cached_map_hashes_the_same() {
        let (p, t, seed) = (params(), test_tuning(), 5u64);
        let (w, nav) = built(seed);
        let bytes = encode(&w, &nav, key(&p, seed, &t));
        let g = from_cache(&bytes, &p, seed, &t, REF_MAX_EDITS).expect("кэш читается");
        let mut a = Reference::with_params(seed, 40, t, test_weapons(), p);
        let mut b = Reference::with_ground(seed, 40, t, test_weapons(), p, g);
        let mut same = 0;
        for tick in 0..50 {
            let (ha, hb) = (a.tick(), b.tick());
            assert_eq!(ha, hb, "тик {tick}: партия на карте из кэша разошлась");
            same += 1;
        }
        assert_eq!(same, 50);
    }

    /// **Чужая карта отвергается ключом**: другой сид — другой ключ.
    #[test]
    fn another_seed_is_another_key() {
        let (p, t) = (params(), test_tuning());
        let (w, nav) = built(3);
        let bytes = encode(&w, &nav, key(&p, 3, &t));
        match decode(&bytes, &p, 4, &t, REF_MAX_EDITS) {
            Err(CacheError::Key { .. }) => {}
            other => panic!("кэш чужого сида принят: {:?}", other.map(|_| ())),
        }
    }

    /// **Содержимое сверяется с генератором, а не только ключ.** Кэш
    /// с верным ключом и чужим объёмом — порча, и читается как порча.
    #[test]
    fn a_wrong_volume_under_the_right_key_is_refused() {
        let (p, t, seed) = (params(), test_tuning(), 9u64);
        let (w, nav) = built(seed);
        let mut bytes = encode(&w, &nav, key(&p, seed, &t));
        let start = MAGIC.len() + 4 + 8 + 12 + 8;
        let words = w.bits().len();
        for b in &mut bytes[start..start + words * 8] {
            *b = !*b;
        }
        assert_eq!(
            decode(&bytes, &p, seed, &t, REF_MAX_EDITS).map(|_| ()),
            Err(CacheError::Mismatch)
        );
    }

    /// **Обрезанный кэш — не паника и не пустая карта.** Каждый срез
    /// обязан читаться как порча; на длинном буфере срезов слишком
    /// много, поэтому объём здесь маленький.
    #[test]
    fn a_truncated_cache_is_refused_at_every_cut() {
        // Мир нарочно крошечный: срезов столько, сколько байт, и на
        // каждом срезе кэш читается заново. Высота — не меньше
        // шестнадцати: генератор держит поверхность в восьми вокселях
        // от дна и от неба.
        let p = worldgen::Params::sized(16, 24, 8);
        let (t, seed) = (test_tuning(), 2u64);
        let w = voxel::World::new(p.nx, p.ny, p.nz, REF_MAX_EDITS, Box::new(worldgen::Terrain { params: p, seed }));
        let nav = NavLayer::new(&w, t.unit_height, t.nav_cell, t.max_step);
        let bytes = encode(&w, &nav, key(&p, seed, &t));
        assert!(decode(&bytes, &p, seed, &t, REF_MAX_EDITS).is_ok(), "целый кэш не читается");
        for cut in 0..bytes.len() {
            let r = decode(&bytes[..cut], &p, seed, &t, REF_MAX_EDITS).map(|_| ());
            assert!(
                matches!(r, Err(CacheError::Truncated) | Err(CacheError::NotACache)),
                "срез на {cut} байте дал {r:?}"
            );
        }
        let mut other = bytes.clone();
        other[MAGIC.len()] ^= 0xFF;
        assert!(matches!(
            decode(&other, &p, seed, &t, REF_MAX_EDITS).map(|_| ()),
            Err(CacheError::Version { .. })
        ));
    }
}
