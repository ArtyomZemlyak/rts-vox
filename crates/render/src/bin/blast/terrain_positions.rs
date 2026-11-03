//! Exact body edit positions, sharing transforms across one removed-cell batch.
use fx::Fx;
use sim_core::{
    blast::RangeUnit,
    damage::{Frame, V3},
};

struct Terms {
    base: V3,
    axes: [Vec<V3>; 3],
}
impl Terms {
    fn new(frame: Frame, size: [usize; 3]) -> Self {
        let basis = [frame.x, frame.y, frame.z];
        Self {
            base: frame.base,
            axes: std::array::from_fn(|a| {
                (0..size[a])
                    .map(|v| {
                        let local = Fx::from_int(v as i32).add(Fx::from_ratio(1, 2));
                        basis[a].scale(local.sub(frame.centre[a]).div(frame.scale))
                    })
                    .collect()
            }),
        }
    }
    fn world(&self, p: [u8; 3]) -> V3 {
        // Same additions, in the same order, as Frame::world.
        self.base
            .add(self.axes[0][usize::from(p[0])])
            .add(self.axes[1][usize::from(p[1])])
            .add(self.axes[2][usize::from(p[2])])
    }
}

pub(super) fn removed(u: &RangeUnit, from: usize) -> impl Iterator<Item = [i32; 3]> + '_ {
    let cells = &u.removed[from.min(u.removed.len())..];
    let split = u.body.turret_y;
    let frames = [
        u.pose,
        if cells.is_empty() {
            u.pose
        } else {
            u.root_frame(usize::from(split))
        },
    ];
    // Small edits share the frame but need no tables. Wide destruction reuses
    // each exact axis term instead of dividing/multiplying it for every cell.
    let terms = (cells.len() >= 32).then(|| {
        let size = [u.body.vol.nx, u.body.vol.ny, u.body.vol.nz].map(usize::from);
        frames.map(|frame| Terms::new(frame, size))
    });
    cells.iter().map(move |&p| {
        let actor = usize::from(p[1] >= split);
        let v = match &terms {
            Some(terms) => terms[actor].world(p),
            None => {
                frames[actor].world(p.map(|v| Fx::from_int(i32::from(v)).add(Fx::from_ratio(1, 2))))
            }
        };
        [v.x, v.y, v.z].map(|v| v.to_int_trunc())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use modding::range::RangeUnitKind;

    #[test]
    fn batch_positions_match_per_voxel_frames_for_every_unit() {
        for kind in RangeUnitKind::ALL {
            let mut u = RangeUnit::of_kind(kind, [17, 23, 31]);
            for z in (0..u.body.vol.nz).step_by(7) {
                for y in (0..u.body.vol.ny).step_by(3) {
                    for x in (0..u.body.vol.nx).step_by(5) {
                        u.removed.push([x, y, z]);
                    }
                }
            }
            for (head, delta, pitch, scale) in [
                (0, 0, 0, Fx::ONE),
                (377, -211, 64, Fx::from_ratio(3, 2)),
                (841, 391, -41, Fx::from_ratio(7, 5)),
            ] {
                u.pose = Frame::new(
                    &u.body,
                    V3::new(
                        Fx::from_ratio(-17, 3),
                        Fx::from_int(23),
                        Fx::from_ratio(47, 7),
                    ),
                    head,
                    V3::new(Fx::from_ratio(1, 10), Fx::ONE, Fx::from_ratio(-1, 5)),
                );
                u.pose.scale = scale;
                u.turret_delta = delta;
                u.turret_pitch = pitch;
                let all = u.removed.clone();
                for count in [0, 1, 31, 32, all.len()] {
                    u.removed = all[..count.min(all.len())].to_vec();
                    for from in [0, u.removed.len() / 2, u.removed.len()] {
                        let expected: Vec<_> = u.removed[from..]
                            .iter()
                            .map(|p| u.world_pos(p.map(usize::from)))
                            .collect();
                        assert_eq!(
                            removed(&u, from).collect::<Vec<_>>(),
                            expected,
                            "kind={kind:?} head={head} count={count} from={from}"
                        );
                    }
                }
                u.removed = all;
                assert!(removed(&u, u.removed.len() + 1).next().is_none());
            }
        }
    }
}
