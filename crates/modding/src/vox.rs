//! Чтение и запись MagicaVoxel `.vox`.
//!
//! Формат выбран потому, что редактор бесплатный и им пользуются: «новый
//! юнит добавляется `.vox`-моделью» перестаёт быть обещанием только если
//! движок читает то, что редактор пишет.
//!
//! **Файл мода — недоверенный вход.** В lockstep он приходит от игрока
//! и грузится у всех, поэтому здесь нет ни одной паники, ни одного
//! выделения по числу из файла и ни одного цикла, длина которого задана
//! файлом без потолка. Битый или злонамеренный файл обязан дать `Err`,
//! а не уронить партию у всех клиентов.
//!
//! **Оси.** MagicaVoxel держит вверх `z`, движок — `y` (`docs/05-units.md`).
//! Перестановка делается **здесь, при чтении**, ровно в одном месте:
//! мир `(x, y, z)` = файл `(x, z, y)`. Ниже по течению про формат уже
//! никто не знает.

use crate::PartTag;

/// Потолки. Юнит — маленький том (ADR-019), и это не ограничение
/// формата, а отказ грузить то, что заведомо не юнит.
pub const MAX_SIDE: u32 = 128;
pub const MAX_VOXELS: u32 = 1 << 20;
/// Потолок на весь файл: 16 МиБ. Больше — не модель юнита.
pub const MAX_FILE: usize = 16 << 20;

#[derive(Debug, PartialEq, Eq)]
pub enum VoxError {
    /// Не `.vox`: нет сигнатуры.
    NotVox,
    /// Версия формата, которую мы не читаем.
    Version(u32),
    /// Файл кончился посреди структуры.
    Truncated,
    /// Размер модели вне разумного.
    Size(u32, u32, u32),
    /// Слишком много вокселей.
    TooManyVoxels(u32),
    /// Файл больше потолка.
    TooBig(usize),
    /// Ни одной модели.
    NoModel,
    /// Воксель вне объявленного размера.
    OutOfBounds,
}

/// Модель как она лежит в файле: размеры и воксели с номером цвета.
/// Смысл цветам придаёт уже RON юнита, а не сам `.vox`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoxModel {
    pub nx: u32,
    pub ny: u32,
    pub nz: u32,
    /// `(x, y, z, номер цвета)` в осях **движка**.
    pub voxels: Vec<(u8, u8, u8, u8)>,
}

struct Cursor<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> Cursor<'a> {
    fn u32(&mut self) -> Result<u32, VoxError> {
        let e = self.i.checked_add(4).ok_or(VoxError::Truncated)?;
        let s = self.b.get(self.i..e).ok_or(VoxError::Truncated)?;
        self.i = e;
        Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    }

    fn tag(&mut self) -> Result<[u8; 4], VoxError> {
        let e = self.i.checked_add(4).ok_or(VoxError::Truncated)?;
        let s = self.b.get(self.i..e).ok_or(VoxError::Truncated)?;
        self.i = e;
        Ok([s[0], s[1], s[2], s[3]])
    }

    fn skip(&mut self, n: usize) -> Result<(), VoxError> {
        let e = self.i.checked_add(n).ok_or(VoxError::Truncated)?;
        if e > self.b.len() {
            return Err(VoxError::Truncated);
        }
        self.i = e;
        Ok(())
    }
}

/// Первая модель файла. Многомодельные `.vox` (сцены) читаются как
/// первая модель: составное тело — отдельное решение (ADR-020), и
/// молча склеивать модели в одну нельзя.
pub fn read(bytes: &[u8]) -> Result<VoxModel, VoxError> {
    if bytes.len() > MAX_FILE {
        return Err(VoxError::TooBig(bytes.len()));
    }
    let mut c = Cursor { b: bytes, i: 0 };
    if c.tag()? != *b"VOX " {
        return Err(VoxError::NotVox);
    }
    let ver = c.u32()?;
    // 150 — то, что пишет редактор с 2015 года; 200 — свежие сборки.
    if ver != 150 && ver != 200 {
        return Err(VoxError::Version(ver));
    }
    if c.tag()? != *b"MAIN" {
        return Err(VoxError::NotVox);
    }
    let main_content = c.u32()? as usize;
    let _children = c.u32()?;
    c.skip(main_content)?;

    let mut size: Option<(u32, u32, u32)> = None;
    let mut model: Option<VoxModel> = None;

    // Обход детей MAIN. Цикл ограничен длиной файла, а не числом
    // из файла: битый размер чанка не даёт зациклиться.
    while c.i + 12 <= bytes.len() && model.is_none() {
        let tag = c.tag()?;
        let content = c.u32()? as usize;
        let children = c.u32()? as usize;
        let start = c.i;
        match &tag {
            b"SIZE" => {
                let (a, b, d) = (c.u32()?, c.u32()?, c.u32()?);
                // Файл держит вверх z, движок — y.
                let (nx, ny, nz) = (a, d, b);
                if nx == 0 || ny == 0 || nz == 0 || nx > MAX_SIDE || ny > MAX_SIDE || nz > MAX_SIDE
                {
                    return Err(VoxError::Size(nx, ny, nz));
                }
                size = Some((nx, ny, nz));
            }
            b"XYZI" => {
                let (nx, ny, nz) = size.ok_or(VoxError::NoModel)?;
                let n = c.u32()?;
                if n > MAX_VOXELS {
                    return Err(VoxError::TooManyVoxels(n));
                }
                // Заявленное число сверяется с тем, что реально лежит
                // в файле, **до** выделения: иначе `n` из файла — это
                // управление нашей памятью.
                let need = (n as usize).checked_mul(4).ok_or(VoxError::Truncated)?;
                if c.i.checked_add(need).ok_or(VoxError::Truncated)? > bytes.len() {
                    return Err(VoxError::Truncated);
                }
                let mut voxels = Vec::with_capacity(n as usize);
                for k in 0..n as usize {
                    let s = &bytes[c.i + k * 4..c.i + k * 4 + 4];
                    let (fx, fy, fz, ci) = (s[0], s[1], s[2], s[3]);
                    let (x, y, z) = (fx, fz, fy);
                    if x as u32 >= nx || y as u32 >= ny || z as u32 >= nz {
                        return Err(VoxError::OutOfBounds);
                    }
                    voxels.push((x, y, z, ci));
                }
                c.i += need;
                model = Some(VoxModel { nx, ny, nz, voxels });
            }
            _ => {}
        }
        // На следующий чанк идём от начала содержимого, а не от того,
        // где остановилось чтение: неизвестные чанки пропускаются целиком.
        c.i = start;
        c.skip(content)?;
        c.skip(children)?;
    }
    model.ok_or(VoxError::NoModel)
}

/// Запись. Нужна не для игры, а для теста кругового хода: читатель,
/// который не с чем сверить, проверяет сам себя.
pub fn write(m: &VoxModel) -> Vec<u8> {
    let mut out = Vec::new();
    let chunk = |out: &mut Vec<u8>, tag: &[u8; 4], data: &[u8]| {
        out.extend_from_slice(tag);
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(data);
    };
    out.extend_from_slice(b"VOX ");
    out.extend_from_slice(&150u32.to_le_bytes());

    let mut kids = Vec::new();
    let mut size = Vec::new();
    // Обратно в оси файла.
    size.extend_from_slice(&m.nx.to_le_bytes());
    size.extend_from_slice(&m.nz.to_le_bytes());
    size.extend_from_slice(&m.ny.to_le_bytes());
    chunk(&mut kids, b"SIZE", &size);

    let mut xyzi = Vec::with_capacity(4 + m.voxels.len() * 4);
    xyzi.extend_from_slice(&(m.voxels.len() as u32).to_le_bytes());
    for &(x, y, z, ci) in &m.voxels {
        xyzi.extend_from_slice(&[x, z, y, ci]);
    }
    chunk(&mut kids, b"XYZI", &xyzi);

    out.extend_from_slice(b"MAIN");
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&(kids.len() as u32).to_le_bytes());
    out.extend_from_slice(&kids);
    out
}

/// Модель плюс раскраска: что означает каждый номер цвета.
/// Номера, которых нет в таблице, считаются бронёй — мод, забывший
/// пометить воксель, получает броню, а не дыру.
pub fn tag_at(palette: &[(u8, PartTag)], colour: u8) -> PartTag {
    for &(c, p) in palette {
        if c == colour {
            return p;
        }
    }
    PartTag::Armour
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> VoxModel {
        let mut voxels = Vec::new();
        for z in 0..3u8 {
            for y in 0..4u8 {
                for x in 0..5u8 {
                    voxels.push((x, y, z, 1 + (x + y + z) % 4));
                }
            }
        }
        VoxModel {
            nx: 5,
            ny: 4,
            nz: 3,
            voxels,
        }
    }

    #[test]
    fn round_trip_is_exact() {
        let m = sample();
        assert_eq!(read(&write(&m)).unwrap(), m);
    }

    /// Ось `y` файла — это `z` движка. Проверяется на несимметричной
    /// модели: на кубе перестановка осей не видна.
    #[test]
    fn axes_are_swapped_exactly_once() {
        let m = VoxModel {
            nx: 7,
            ny: 3,
            nz: 5,
            voxels: vec![(6, 2, 4, 9)],
        };
        let bytes = write(&m);
        // В файле стороны лежат как (x, z, y) = (7, 5, 3).
        let back = read(&bytes).unwrap();
        assert_eq!((back.nx, back.ny, back.nz), (7, 3, 5));
        assert_eq!(back.voxels, vec![(6, 2, 4, 9)]);
    }

    #[test]
    fn garbage_is_an_error_not_a_panic() {
        assert_eq!(read(b""), Err(VoxError::Truncated));
        assert_eq!(read(b"NOPE\x96\x00\x00\x00"), Err(VoxError::NotVox));
        let mut good = write(&sample());
        for cut in (0..good.len()).step_by(3) {
            // Любой обрезок обязан дать Err, а не панику.
            let _ = read(&good[..cut]);
        }
        // Мусор на месте каждого байта — тоже.
        for i in 0..good.len() {
            let old = good[i];
            good[i] = old.wrapping_add(0x5A);
            let _ = read(&good);
            good[i] = old;
        }
    }

    /// Заявленное число вокселей не имеет права управлять нашей памятью.
    #[test]
    fn a_lying_voxel_count_does_not_allocate() {
        let mut b = write(&sample());
        // Найти XYZI и подменить счётчик на четыре миллиарда.
        let at = b
            .windows(4)
            .position(|w| w == b"XYZI")
            .expect("в файле есть XYZI");
        let n = at + 12;
        b[n..n + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(read(&b), Err(VoxError::TooManyVoxels(u32::MAX)));
    }

    #[test]
    fn a_voxel_outside_the_declared_size_is_an_error() {
        let m = VoxModel {
            nx: 2,
            ny: 2,
            nz: 2,
            voxels: vec![(0, 0, 0, 1)],
        };
        let mut b = write(&m);
        let at = b.windows(4).position(|w| w == b"XYZI").unwrap();
        b[at + 16] = 200; // x = 200 при nx = 2
        assert_eq!(read(&b), Err(VoxError::OutOfBounds));
    }

    #[test]
    fn unknown_chunks_are_skipped() {
        let m = sample();
        let mut b = write(&m);
        // Вставить неизвестный чанк перед SIZE.
        let at = b.windows(4).position(|w| w == b"SIZE").unwrap();
        let mut junk = Vec::new();
        junk.extend_from_slice(b"nTRN");
        junk.extend_from_slice(&8u32.to_le_bytes());
        junk.extend_from_slice(&0u32.to_le_bytes());
        junk.extend_from_slice(&[0xAB; 8]);
        b.splice(at..at, junk);
        assert_eq!(read(&b).unwrap(), m);
    }
}
