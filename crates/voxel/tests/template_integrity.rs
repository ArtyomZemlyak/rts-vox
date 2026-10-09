//! Inspect the actual occupied volumes, including every articulated part.
use std::collections::VecDeque;
use voxel::{topo::unit::UnitVol, UnitBody};

fn templates() -> Vec<(&'static str, UnitBody)> {
    let mut all = voxel::roster::all();
    all.extend([
        ("range walker", voxel::unit::strider()),
        ("range spider", voxel::unit::spider()),
        ("range artillery", voxel::unit::artillery()),
        ("range artillery T2", voxel::unit::artillery_t2()),
        (
            "range artillery turret T2",
            voxel::unit::artillery_turret_t2(),
        ),
        ("range turret T1", voxel::unit::turret_t1()),
        ("range turret T2", voxel::unit::turret_t2()),
        ("range command base", voxel::unit::command_base()),
        ("biped", voxel::unit::biped()),
    ]);
    all
}

#[derive(Debug)]
struct Pocket {
    cells: usize,
    min: [usize; 3],
    max: [usize; 3],
}

fn neighbours(i: usize, n: [usize; 3], mut visit: impl FnMut(usize)) {
    let p = [i % n[0], i / n[0] % n[1], i / (n[0] * n[1])];
    for axis in 0..3 {
        let stride = [1, n[0], n[0] * n[1]][axis];
        if p[axis] > 0 {
            visit(i - stride);
        }
        if p[axis] + 1 < n[axis] {
            visit(i + stride);
        }
    }
}

/// Six-face flood from the outside, followed by an independent flood of
/// each enclosed void. Exterior slots between feet are not enclosed holes.
fn pockets(v: &UnitVol) -> Vec<Pocket> {
    let n = [v.nx as usize, v.ny as usize, v.nz as usize];
    let mut seen = vec![0u8; n.iter().product()];
    let mut queue = VecDeque::new();
    for z in 0..n[2] {
        for y in 0..n[1] {
            for x in 0..n[0] {
                let i = (z * n[1] + y) * n[0] + x;
                if v.get(x, y, z) {
                    seen[i] = 1;
                } else if [x, y, z]
                    .iter()
                    .zip(n)
                    .any(|(&p, len)| p == 0 || p + 1 == len)
                {
                    seen[i] = 2;
                    queue.push_back(i);
                }
            }
        }
    }
    while let Some(i) = queue.pop_front() {
        neighbours(i, n, |j| {
            if seen[j] == 0 {
                seen[j] = 2;
                queue.push_back(j);
            }
        });
    }
    let mut result = Vec::new();
    for start in 0..seen.len() {
        if seen[start] != 0 {
            continue;
        }
        let mut pocket = Pocket {
            cells: 0,
            min: [usize::MAX; 3],
            max: [0; 3],
        };
        seen[start] = 3;
        queue.push_back(start);
        while let Some(i) = queue.pop_front() {
            let p = [i % n[0], i / n[0] % n[1], i / (n[0] * n[1])];
            pocket.cells += 1;
            for (axis, &v) in p.iter().enumerate() {
                pocket.min[axis] = pocket.min[axis].min(v);
                pocket.max[axis] = pocket.max[axis].max(v);
            }
            neighbours(i, n, |j| {
                if seen[j] == 0 {
                    seen[j] = 3;
                    queue.push_back(j);
                }
            });
        }
        result.push(pocket);
    }
    result
}

#[test]
fn every_stock_template_and_segment_has_no_enclosed_air() {
    let mut failures = Vec::new();
    let mut parts = 0;
    let models = templates();
    let count = models.len();
    for (name, body) in models {
        let st = body.status(&mut Default::default());
        assert!(st.alive, "{name}: missing core");
        assert_eq!(st.detached, 0, "{name}: disconnected root geometry");
        for (segment, part) in std::iter::once((None, &body)).chain(
            body.segments
                .iter()
                .enumerate()
                .map(|(i, s)| (Some(i), &s.body)),
        ) {
            parts += 1;
            let holes = pockets(&part.vol);
            if !holes.is_empty() {
                failures.push(format!("{name} segment={segment:?}: {holes:?}"));
            }
        }
    }
    eprintln!("inspected {parts} root/segment volumes across {count} stock templates");
    assert!(
        failures.is_empty(),
        "enclosed voids:\n{}",
        failures.join("\n")
    );
}

#[test]
fn detector_distinguishes_closed_cavities_from_exterior_gaps() {
    let mut v = UnitVol::new(5, 5, 5);
    for z in 0..5 {
        for y in 0..5 {
            for x in 0..5 {
                v.set(x, y, z, true);
            }
        }
    }
    assert!(pockets(&v).is_empty());
    v.set(2, 2, 2, false);
    let result = pockets(&v);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].cells, 1);
    assert_eq!(result[0].min, [2; 3]);
    v.set(2, 2, 1, false);
    v.set(2, 2, 0, false);
    assert!(pockets(&v).is_empty());
}

#[test]
fn each_articulated_part_is_a_single_connected_solid() {
    let mut failures = Vec::new();
    for (name, body) in templates() {
        for (segment, part) in body.segments.iter().enumerate() {
            let v = &part.body.vol;
            let n = [v.nx as usize, v.ny as usize, v.nz as usize];
            let mut seen = vec![false; n.iter().product()];
            let mut queue = VecDeque::new();
            let mut components = Vec::new();
            for start in 0..seen.len() {
                let solid = |i: usize| v.get(i % n[0], i / n[0] % n[1], i / (n[0] * n[1]));
                if seen[start] || !solid(start) {
                    continue;
                }
                seen[start] = true;
                queue.push_back(start);
                let mut count = 0;
                while let Some(i) = queue.pop_front() {
                    count += 1;
                    neighbours(i, n, |j| {
                        if !seen[j] && solid(j) {
                            seen[j] = true;
                            queue.push_back(j);
                        }
                    });
                }
                components.push(count);
            }
            if components.len() != 1 {
                failures.push(format!("{name} segment={segment}: {components:?}"));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "disconnected parts:\n{}",
        failures.join("\n")
    );
}
