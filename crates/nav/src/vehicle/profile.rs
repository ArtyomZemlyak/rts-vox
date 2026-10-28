//! Common terrain profiles fit inline; arbitrary mod profiles remain supported.
use voxel::store::Mat;

pub(super) enum Yielding {
    Inline { values: [Mat; 2], len: usize },
    Many(Vec<Mat>),
}
impl Yielding {
    pub fn new(materials: &[Mat]) -> Self {
        if materials.len() <= 2 {
            let mut values = [0; 2];
            values[..materials.len()].copy_from_slice(materials);
            Self::Inline {
                values,
                len: materials.len(),
            }
        } else {
            Self::Many(materials.to_vec())
        }
    }
    pub fn as_slice(&self) -> &[Mat] {
        match self {
            Self::Inline { values, len } => &values[..*len],
            Self::Many(values) => values,
        }
    }
}
impl std::ops::Deref for Yielding {
    type Target = [Mat];
    fn deref(&self) -> &[Mat] {
        self.as_slice()
    }
}

/// Same centre and four square rings, in dz/dx order. Coordinates are added
/// only for candidates actually visited by the bounded goal probe.
pub(super) const GOAL_OFFSETS: [[i32; 2]; 81] = {
    let mut out = [[0; 2]; 81];
    let mut index = 0;
    let mut r = 0i32;
    while r <= 4 {
        let mut dz = -r;
        while dz <= r {
            let mut dx = -r;
            while dx <= r {
                if dx == -r || dx == r || dz == -r || dz == r {
                    out[index] = [dx, dz];
                    index += 1;
                }
                dx += 1;
            }
            dz += 1;
        }
        r += 1;
    }
    out
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profiles_preserve_empty_single_pair_and_arbitrary_materials() {
        for count in [0, 1, 2, 3, 33, 257] {
            let input: Vec<Mat> = (0..count).map(|i| (i * 251) as Mat).collect();
            let stored = Yielding::new(&input);
            assert_eq!(stored.as_slice(), input);
            assert_eq!(matches!(stored, Yielding::Inline { .. }), count <= 2);
        }
        assert_eq!(Yielding::new(&[u16::MAX, 0]).as_slice(), &[u16::MAX, 0]);
    }

    #[test]
    fn goal_candidates_keep_the_original_ring_and_tie_order() {
        let mut reference = Vec::new();
        for r in 0..=4i32 {
            for dz in -r..=r {
                for dx in -r..=r {
                    if dx.abs() == r || dz.abs() == r {
                        reference.push([dx, dz]);
                    }
                }
            }
        }
        assert_eq!(GOAL_OFFSETS.as_slice(), reference);
    }
}
