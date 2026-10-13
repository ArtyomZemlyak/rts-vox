//! Immutable exact geometry for the small masks and ordinary stacked piles.
//! No fragment state or cross-tick cache: all entries are integer constants.
use fx::Fx;

pub(super) struct Geometry {
    pub centre: [Fx; 3],
    pub bounds: ([Fx; 3], [Fx; 3]),
}

const EMPTY: Geometry = Geometry {
    centre: [Fx::ZERO; 3],
    bounds: ([Fx::ZERO; 3], [Fx::ZERO; 3]),
};

const fn geometry(shape: u64, side: usize) -> Geometry {
    let mut sums = [0; 3];
    let mut low = [side as i32 - 1; 3];
    let mut high = [0; 3];
    let mut bit = 0;
    while bit < side * side * side {
        if shape & (1u64 << bit) != 0 {
            let at = [bit % side, bit / side % side, bit / (side * side)];
            let mut a = 0;
            while a < 3 {
                let value = at[a] as i32;
                sums[a] += value;
                if value < low[a] {
                    low[a] = value;
                }
                if value > high[a] {
                    high[a] = value;
                }
                a += 1;
            }
        }
        bit += 1;
    }
    let count = shape.count_ones();
    let denominator = if count == 0 { 1 } else { count as i32 };
    let mut out = EMPTY;
    let mut a = 0;
    while a < 3 {
        let centre = Fx::from_ratio(sums[a], denominator);
        out.centre[a] = centre;
        out.bounds.0[a] = Fx::from_int(low[a])
            .sub(centre)
            .sub(Fx::from_ratio(49, 100));
        out.bounds.1[a] = Fx::from_int(high[a])
            .sub(centre)
            .add(Fx::from_ratio(49, 100));
        a += 1;
    }
    out
}

const SINGLE: [Geometry; 2] = [geometry(0, 1), geometry(1, 1)];
const SMALL: [Geometry; 256] = {
    let mut out = [const { EMPTY }; 256];
    let mut shape = 0;
    while shape < out.len() {
        out[shape] = geometry(shape as u64, 2);
        shape += 1;
    }
    out
};

pub(super) const STACKED: [u64; 65] = {
    let mut out = [0; 65];
    let mut count = 0;
    while count < 64 {
        // restack fills X, then Z, then Y; shape bits are X + 4Y + 16Z.
        let bit = count % 4 + 4 * (count / 16) + 16 * (count / 4 % 4);
        out[count + 1] = out[count] | (1u64 << bit);
        count += 1;
    }
    out
};
const PILES: [Geometry; 65] = {
    let mut out = [const { EMPTY }; 65];
    let mut count = 0;
    while count < out.len() {
        out[count] = geometry(STACKED[count], 4);
        count += 1;
    }
    out
};

pub(super) fn known(shape: u64, side: u8) -> Option<&'static Geometry> {
    match side.clamp(1, 4) {
        1 if shape < 2 => Some(&SINGLE[shape as usize]),
        2 if shape < 256 => Some(&SMALL[shape as usize]),
        4 => {
            let count = shape.count_ones() as usize;
            (STACKED[count] == shape).then(|| &PILES[count])
        }
        _ => None,
    }
}
