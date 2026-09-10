//! Построение осевых масок из **готового битового объёма**, а не из массива
//! материалов.
//!
//! Мотив: замер показал, что построение масок — 99 мкс из 173 на реальном
//! чанке, то есть **больше половины мешинга**, и оно не зависит от содержимого.
//! При этом ровно такая же битовая маска занятости уже существует по ADR-061
//! для марша лучей. Строить её дважды — чистая потеря.
//!
//! Вопрос, который здесь измеряется: одна раскладка бит обслуживает марш,
//! но мешеру нужны колонки по трём осям. Перекладка — это 128 транспозиций
//! битовой матрицы 64×64, и вопрос в том, дешевле ли они, чем обход байтов.

use crate::CSP;

/// Битовый объём чанка в раскладке `[y][z]` → колонка по X.
/// Ровно то, что даёт L0 битового объёма при совпадающей раскладке.
pub type BitVolume = [u64; CSP * CSP];

/// Транспонирование битовой матрицы 64×64 на месте.
/// Рекурсивная схема из Hacker's Delight: шесть уровней масок,
/// 64 слова, без таблиц и без ветвлений внутри.
pub fn transpose64(a: &mut [u64; 64]) {
    let mut j = 32usize;
    let mut m: u64 = 0x0000_0000_FFFF_FFFF;
    while j != 0 {
        let mut k = 0usize;
        while k < 64 {
            let t = ((a[k] >> j) ^ a[k + j]) & m;
            a[k + j] ^= t;
            a[k] ^= t << j;
            k = (k + j + 1) & !j;
        }
        j >>= 1;
        m ^= m << j;
    }
}

/// Разложить битовый объём в три набора осевых колонок.
///
/// Соглашения те же, что у мешера:
/// - `cols[0][z*64 + y]` — биты по X;
/// - `cols[1][x*64 + z]` — биты по Y;
/// - `cols[2][y*64 + x]` — биты по Z.
pub fn expand_axes(bits: &BitVolume, cols: &mut [u64]) {
    debug_assert!(cols.len() >= 3 * CSP * CSP);

    // Ось X — чистая перестановка индексов, без битовых операций.
    for y in 0..CSP {
        for z in 0..CSP {
            cols[z * CSP + y] = bits[y * CSP + z];
        }
    }

    // Ось Z: при фиксированном y транспонируем матрицу [z][x] в [x][z].
    let mut buf = [0u64; 64];
    for y in 0..CSP {
        buf.copy_from_slice(&bits[y * CSP..(y + 1) * CSP]);
        transpose64(&mut buf);
        // После транспозиции buf[x] содержит биты по z.
        cols[2 * CSP * CSP + y * CSP..2 * CSP * CSP + (y + 1) * CSP].copy_from_slice(&buf);
    }

    // Ось Y: при фиксированном z транспонируем [y][x] в [x][y].
    for z in 0..CSP {
        for y in 0..CSP {
            buf[y] = bits[y * CSP + z];
        }
        transpose64(&mut buf);
        // buf[x] содержит биты по y.
        for x in 0..CSP {
            cols[CSP * CSP + x * CSP + z] = buf[x];
        }
    }
}

/// Собрать битовый объём из массива материалов. В движке этого шага нет —
/// объём уже есть; здесь он нужен, чтобы сверять два пути между собой.
pub fn bits_from_chunk(c: &crate::Chunk) -> BitVolume {
    let mut b = [0u64; CSP * CSP];
    for y in 0..CSP {
        for z in 0..CSP {
            let mut w = 0u64;
            let row = (y * CSP + z) * CSP;
            for x in 0..CSP {
                if c.v[row + x] != crate::AIR {
                    w |= 1u64 << x;
                }
            }
            b[y * CSP + z] = w;
        }
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transpose_is_an_involution() {
        let mut a = [0u64; 64];
        let mut r = fx::rng::Rng::new(1, 2);
        for v in a.iter_mut() {
            *v = r.next_u64();
        }
        let orig = a;
        transpose64(&mut a);
        assert_ne!(a, orig, "транспозиция ничего не сделала");
        transpose64(&mut a);
        assert_eq!(a, orig, "двойная транспозиция обязана вернуть исходное");
    }

    #[test]
    fn transpose_moves_the_right_bit() {
        let mut a = [0u64; 64];
        a[3] = 1u64 << 17;
        transpose64(&mut a);
        assert_eq!(a[17], 1u64 << 3);
    }

    /// Оси, полученные транспозицией, обязаны совпасть с осями, построенными
    /// обходом байтов. Это единственное, что делает быстрый путь пригодным.
    #[test]
    fn expand_axes_matches_byte_scan() {
        for scene in crate::scene::Scene::ALL {
            let c = crate::scene::build(scene, 7);
            let mut slow = vec![0u64; 3 * CSP * CSP];
            for y in 0..CSP {
                for z in 0..CSP {
                    for x in 0..CSP {
                        if c.v[crate::Chunk::idx(x, y, z)] == crate::AIR {
                            continue;
                        }
                        slow[z * CSP + y] |= 1u64 << x;
                        slow[CSP * CSP + x * CSP + z] |= 1u64 << y;
                        slow[2 * CSP * CSP + y * CSP + x] |= 1u64 << z;
                    }
                }
            }
            let bits = bits_from_chunk(&c);
            let mut fast = vec![0u64; 3 * CSP * CSP];
            expand_axes(&bits, &mut fast);
            assert_eq!(fast, slow, "сцена {}", scene.name());
        }
    }
}
