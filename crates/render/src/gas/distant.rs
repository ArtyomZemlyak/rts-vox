//! Compact distant columns and acknowledged, immutable GPU upload patches.
//! Four solid spans retain empty space below foliage and roofs, and preserve
//! vertical voxel precision. This is presentation data, never collision data.
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

pub const MAX_SIDE: u32 = 2560;
pub const SPANS: usize = 4;
pub const MAX_COLUMN_WORDS: usize = SPANS * 2;
pub const WORDS: usize = MAX_SIDE as usize * MAX_SIDE as usize * MAX_COLUMN_WORDS;

pub fn column_words(height: u32) -> usize {
    SPANS * if height > 255 { 2 } else { 1 }
}

#[derive(Clone, Default)]
pub struct Uploads {
    pub revision: u64,
    pub patches: Vec<(usize, Arc<[u32]>)>,
    pub acknowledged: Arc<AtomicU64>,
}
impl Uploads {
    pub fn retire(&mut self) {
        if self.acknowledged.load(Ordering::Acquire) == self.revision {
            self.patches.clear();
        }
    }
    pub fn push(&mut self, offset: usize, words: &[u32]) {
        self.retire();
        self.patches.push((offset, Arc::from(words)));
        self.revision += 1;
    }
}

pub fn grid(size: [u32; 3]) -> [u32; 3] {
    let step = size[0].max(size[2]).div_ceil(MAX_SIDE).max(4);
    [size[0].div_ceil(step), size[2].div_ceil(step), step]
}

pub fn encode(column: &[u16]) -> [u32; MAX_COLUMN_WORDS] {
    assert!(column.len() <= u16::MAX as usize);
    // [bottom, exclusive top, body material, cap material, cap bottom].
    let mut runs: Vec<[u32; 5]> = Vec::new();
    for (y, &mat) in column.iter().enumerate() {
        if mat == 0 {
            continue;
        }
        let y = y as u32;
        let mat = if mat == 10 {
            8
        } else if mat == 12 {
            11
        } else {
            u32::from(mat).min(14)
        };
        if let Some(last) = runs.last_mut() {
            let ground = |m| matches!(m, 1 | 7 | 8);
            if last[1] == y && (last[3] == mat || (ground(last[2]) && ground(mat))) {
                if last[3] != mat {
                    last[4] = y;
                }
                last[1] = y + 1;
                last[3] = mat;
                continue;
            }
        }
        runs.push([y, y + 1, mat, mat, y]);
    }
    while runs.len() > SPANS {
        // Prefer losing a material boundary inside solid matter, never fill
        // the air below a canopy to save a slot.
        if let Some(i) = (0..runs.len() - 1)
            .filter(|&i| runs[i][1] == runs[i + 1][0])
            .min_by_key(|&i| runs[i + 1][1] - runs[i][0])
        {
            let top = runs.remove(i + 1);
            runs[i][1] = top[1];
            runs[i][3] = top[3];
            runs[i][4] = top[4];
        } else {
            let i = (0..runs.len())
                .min_by_key(|&i| runs[i][1] - runs[i][0])
                .unwrap();
            runs.remove(i);
        }
    }
    let mut words = [0; MAX_COLUMN_WORDS];
    for (i, r) in runs.into_iter().enumerate() {
        if column.len() > 255 {
            // Exact u16 heights; retain both materials and their boundary.
            words[i * 2] = r[0] | r[1] << 16;
            words[i * 2 + 1] = r[2] | r[3] << 4 | r[4] << 8;
        } else {
            words[i] = r[0] | r[1] << 8 | r[2] << 16 | r[3] << 20 | r[4] << 24;
        }
    }
    words
}

#[cfg(test)]
mod tests {
    use super::*;
    fn material(words: [u32; MAX_COLUMN_WORDS], height: u32, y: u32) -> u32 {
        for i in 0..SPANS {
            let [bottom, top, body, cap, boundary] = if height > 255 {
                let lo = words[i * 2];
                let hi = words[i * 2 + 1];
                [lo & 65535, lo >> 16, hi & 15, (hi >> 4) & 15, hi >> 8]
            } else {
                let w = words[i];
                [
                    w & 255,
                    (w >> 8) & 255,
                    (w >> 16) & 15,
                    (w >> 20) & 15,
                    w >> 24,
                ]
            };
            if y >= bottom && y < top {
                return if y >= boundary { cap } else { body };
            }
        }
        0
    }
    #[test]
    fn physical_sand_keeps_ochre_presentation_in_distant_columns() {
        let mut column = [0; 192];
        column[..20].fill(1);
        column[20..24].fill(10);
        let words = encode(&column);
        for y in 20..24 {
            assert_eq!(material(words, 192, y), 8);
        }
    }
    #[test]
    fn ground_surface_roof_and_empty_space_keep_exact_heights() {
        let mut column = [0; 192];
        column[..20].fill(1);
        column[20] = 7;
        column[39..41].fill(3);
        column[62..78].fill(4);
        let words = encode(&column);
        for (y, m) in column.into_iter().enumerate() {
            assert_eq!(material(words, 192, y as u32), u32::from(m));
        }
    }
    #[test]
    fn deep_editor_columns_preserve_top_voxels_caves_and_material_boundaries() {
        for height in [255, 256, 448, 16384] {
            let mut column = vec![0; height];
            let ground = if height == 448 { 272 } else { 80 };
            column[..ground - 1].fill(1);
            column[ground - 1] = 7;
            column[height - 12..height - 10].fill(3);
            column[height - 6..].fill(4);
            let words = encode(&column);
            for (y, &m) in column.iter().enumerate() {
                assert_eq!(
                    material(words, height as u32, y as u32),
                    u32::from(m),
                    "height={height}, y={y}"
                );
            }
            assert_eq!(material(words, height as u32, height as u32), 0);
        }
        assert_eq!(column_words(255), 4);
        assert_eq!(column_words(256), 8);
    }
    #[test]
    fn map_sizes_have_bounded_storage_and_cover_the_last_column() {
        for size in [
            [10_000, 192, 10_000],
            [100_000, 192, 100_000],
            [1281, 192, 2499],
        ] {
            let [nx, nz, step] = grid(size);
            assert!(nx <= MAX_SIDE && nz <= MAX_SIDE);
            assert!(nx * step >= size[0] && nz * step >= size[2]);
            assert!((nx as usize * nz as usize * column_words(size[1])) <= WORDS);
        }
        assert_eq!(grid([10_000, 192, 10_000]), [2500, 2500, 4]);
    }
    #[test]
    fn pending_uploads_survive_delayed_render_frames() {
        let mut u = Uploads::default();
        u.push(0, &[7; 4]);
        let extracted = u.clone();
        u.push(4, &[8; 4]);
        extracted
            .acknowledged
            .store(extracted.revision, Ordering::Release);
        u.retire();
        assert_eq!(u.patches.len(), 2);
        u.acknowledged.store(u.revision, Ordering::Release);
        u.retire();
        assert!(u.patches.is_empty());
    }
}
