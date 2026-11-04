//! Export authoritative posed surfaces for offline model QA, without a window.
use fx::{angle, Fx};
use modding::range::RangeUnitKind;
use sim_core::{blast::RangeUnit, damage::Frame};
use std::{
    fs::File,
    io::{BufWriter, Write},
    path::Path,
};
use voxel::UnitBody;

fn surface(
    body: &UnitBody,
    frame: impl Fn(usize) -> Frame,
    out: &mut impl Write,
    first: &mut bool,
) {
    let size = [body.vol.nx, body.vol.ny, body.vol.nz].map(usize::from);
    let faces = [
        ([1, 0, 0], [[1, 0, 0], [1, 1, 0], [1, 1, 1], [1, 0, 1]]),
        ([-1, 0, 0], [[0, 0, 1], [0, 1, 1], [0, 1, 0], [0, 0, 0]]),
        ([0, 1, 0], [[0, 1, 1], [1, 1, 1], [1, 1, 0], [0, 1, 0]]),
        ([0, -1, 0], [[0, 0, 0], [1, 0, 0], [1, 0, 1], [0, 0, 1]]),
        ([0, 0, 1], [[1, 0, 1], [1, 1, 1], [0, 1, 1], [0, 0, 1]]),
        ([0, 0, -1], [[0, 0, 0], [0, 1, 0], [1, 1, 0], [1, 0, 0]]),
    ];
    for z in 0..size[2] {
        for y in 0..size[1] {
            for x in 0..size[0] {
                if !body.vol.get(x, y, z) {
                    continue;
                }
                for (dir, corners) in faces {
                    let at = [x as i32 + dir[0], y as i32 + dir[1], z as i32 + dir[2]];
                    if (0..3).all(|a| at[a] >= 0 && at[a] < size[a] as i32)
                        && frame(y) == frame(at[1] as usize)
                        && body.vol.get(at[0] as usize, at[1] as usize, at[2] as usize)
                    {
                        continue;
                    }
                    if !*first {
                        write!(out, ",").unwrap();
                    }
                    *first = false;
                    write!(out, "[{}", body.finish_at(x, y, z)).unwrap();
                    let f = frame(y);
                    for v in corners {
                        let p = f.world([
                            Fx::from_int(x as i32 + v[0]),
                            Fx::from_int(y as i32 + v[1]),
                            Fx::from_int(z as i32 + v[2]),
                        ]);
                        write!(
                            out,
                            ",[{:.6},{:.6},{:.6}]",
                            p.x.to_f64_lossy() * 0.1,
                            -p.z.to_f64_lossy() * 0.1,
                            p.y.to_f64_lossy() * 0.1
                        )
                        .unwrap();
                    }
                    write!(out, "]").unwrap();
                }
            }
        }
    }
}

fn main() {
    let destination = std::env::args().nth(1).expect("output directory");
    std::fs::create_dir_all(&destination).unwrap();
    for (name, targets, pitch) in [
        ("rest", None, 0),
        (
            "independent",
            Some([
                [1000, 80, 0],
                [0, 100, 1000],
                [-1000, 60, 0],
                [0, 180, -1000],
            ]),
            0,
        ),
        ("focus", Some([[1000, 60, 0]; 4]), angle::QUARTER),
    ] {
        let mut u = RangeUnit::of_kind(RangeUnitKind::Walker04, [0, 0, 0]);
        u.turret_pitch = pitch;
        if let Some(targets) = targets {
            u.set_arm_targets(targets, 0);
        }
        let mut out = BufWriter::new(
            File::create(Path::new(&destination).join(format!("{name}.json"))).unwrap(),
        );
        write!(out, "[").unwrap();
        let mut first = true;
        surface(
            &u.body,
            |y| {
                if y < usize::from(u.body.turret_y) {
                    u.pose
                } else {
                    sim_core::damage::elevated(
                        &u.pose,
                        pitch,
                        Fx::from_int(i32::from(u.body.turret_y)),
                    )
                }
            },
            &mut out,
            &mut first,
        );
        for (segment, f) in u.body.segments.iter().zip(&u.seg_frames) {
            surface(&segment.body, |_| *f, &mut out, &mut first);
        }
        write!(out, "]").unwrap();
    }
}
