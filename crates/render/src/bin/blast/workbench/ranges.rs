//! Visible boundary of the union of tactical range disks, presentation only.
use bevy::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Circle {
    pub centre: Vec2,
    pub radius: f32,
}
#[derive(Clone, Copy, Debug)]
pub(super) struct Arc {
    pub circle: Circle,
    pub start: f32,
    pub end: f32,
}
#[derive(Default)]
pub(crate) struct Outline {
    circles: Vec<Circle>,
    arcs: Vec<Arc>,
}
impl Outline {
    pub(super) fn update(&mut self, circles: Vec<Circle>) -> &[Arc] {
        if self.circles != circles {
            self.arcs = boundary(&circles);
            self.circles = circles;
        }
        &self.arcs
    }
}

fn boundary(circles: &[Circle]) -> Vec<Arc> {
    use std::f32::consts::TAU;
    let mut arcs = Vec::new();
    let mut covered = Vec::<(f32, f32)>::new();
    for (i, &circle) in circles.iter().enumerate() {
        if circle.radius <= 0.0 || !circle.radius.is_finite() || !circle.centre.is_finite() {
            continue;
        }
        covered.clear();
        let mut hidden = false;
        for (j, other) in circles.iter().enumerate() {
            if i == j || other.radius <= 0.0 {
                continue;
            }
            let delta = other.centre - circle.centre;
            let distance = delta.length();
            if distance == 0.0 && circle.radius == other.radius {
                // Coincident boundaries have one stable owner.
                hidden = j < i;
                if hidden {
                    break;
                }
                continue;
            }
            if distance + circle.radius <= other.radius {
                hidden = true;
                break;
            }
            if distance >= circle.radius + other.radius || distance + other.radius <= circle.radius
            {
                continue;
            }
            let angle = delta.y.atan2(delta.x).rem_euclid(TAU);
            let half = ((distance * distance + circle.radius * circle.radius
                - other.radius * other.radius)
                / (2.0 * distance * circle.radius))
                .clamp(-1.0, 1.0)
                .acos();
            let (start, end) = (angle - half, angle + half);
            if start < 0.0 {
                covered.push((0.0, end));
                covered.push((start + TAU, TAU));
            } else if end > TAU {
                covered.push((start, TAU));
                covered.push((0.0, end - TAU));
            } else {
                covered.push((start, end));
            }
        }
        if hidden {
            continue;
        }
        covered.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
        let mut start = 0.0;
        for &(lo, hi) in &covered {
            if lo > start {
                arcs.push(Arc {
                    circle,
                    start,
                    end: lo,
                });
            }
            start = start.max(hi);
        }
        if start < TAU {
            arcs.push(Arc {
                circle,
                start,
                end: TAU,
            });
        }
    }
    arcs
}

#[cfg(test)]
mod tests {
    use super::*;
    fn disk(x: f32, y: f32, radius: f32) -> Circle {
        Circle {
            centre: Vec2::new(x, y),
            radius,
        }
    }
    fn span(arcs: &[Arc]) -> f32 {
        arcs.iter().map(|a| a.end - a.start).sum()
    }
    #[test]
    fn union_removes_inner_edges_without_filling_gaps() {
        use std::f32::consts::{PI, TAU};
        assert!((span(&boundary(&[disk(0.0, 0.0, 10.0)])) - TAU).abs() < 0.0001);
        assert!(
            (span(&boundary(&[disk(0.0, 0.0, 10.0), disk(10.0, 0.0, 10.0)])) - 8.0 * PI / 3.0)
                .abs()
                < 0.0001
        );
        let disjoint = boundary(&[disk(-30.0, 0.0, 10.0), disk(30.0, 0.0, 10.0)]);
        assert_eq!(disjoint.len(), 2);
        assert!((span(&disjoint) - 2.0 * TAU).abs() < 0.0001);
        for circles in [
            vec![disk(0.0, 0.0, 10.0), disk(0.0, 0.0, 10.0)],
            vec![disk(0.0, 0.0, 10.0), disk(0.0, 0.0, 3.0)],
            vec![disk(0.0, 0.0, 10.0), disk(7.0, 0.0, 3.0)],
        ] {
            let arcs = boundary(&circles);
            assert_eq!(arcs.len(), 1);
            assert_eq!(arcs[0].circle.radius, 10.0);
        }
    }
    #[test]
    fn overlapping_groups_emit_only_uncovered_arcs_and_keep_holes() {
        let circles = [
            disk(-9.0, 0.0, 8.0),
            disk(9.0, 0.0, 8.0),
            disk(0.0, 15.0, 10.0),
            disk(0.0, -15.0, 10.0),
        ];
        let arcs = boundary(&circles);
        assert!(arcs
            .iter()
            .any(|a| a.start > 0.0 || a.end < std::f32::consts::TAU));
        for arc in arcs {
            for t in [0.1, 0.5, 0.9] {
                let angle = arc.start + (arc.end - arc.start) * t;
                let p = arc.circle.centre + Vec2::from_angle(angle) * arc.circle.radius;
                assert!(circles
                    .iter()
                    .all(|c| *c == arc.circle || p.distance(c.centre) >= c.radius - 0.0001));
            }
        }
        assert!(circles.iter().all(|c| c.centre.length() > c.radius));
    }
    #[test]
    fn changed_selection_positions_and_ranges_invalidate_cached_outline() {
        let mut outline = Outline::default();
        let circles = vec![disk(0.0, 0.0, 10.0)];
        outline.update(circles.clone());
        let allocation = outline.arcs.as_ptr();
        assert_eq!(outline.update(circles).as_ptr(), allocation);
        assert_eq!(
            outline.update(vec![disk(1.0, 0.0, 11.0)])[0].circle.radius,
            11.0
        );
        assert!(outline.update(Vec::new()).is_empty());
    }
}
