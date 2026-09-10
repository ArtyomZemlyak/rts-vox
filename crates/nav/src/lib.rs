//! Навигация: решение уравнения эйконала на секторе и производные от него.
//!
//! Крейт существует ради замеров: числа в `docs/10-budgets.md` и `ADR-058`
//! получены оценкой, а не измерением, и расходились между собой в семь раз.

#![forbid(unsafe_code)]

pub mod godunov;
pub mod scene;
pub mod solver;
pub mod vision;

#[cfg(test)]
mod tests {
    use crate::godunov::{h_exact_floor, INF};
    use crate::scene::{build, Scene};
    use crate::solver::*;

    fn goals_center(g: &crate::scene::Grid) -> Vec<usize> {
        // Ближайшая проходимая к центру; если сцена — подземелье, центр
        // может оказаться в породе.
        let (cx, cy) = ((g.w - 1) / 2, (g.h - 1) / 2);
        let mut best = None;
        let mut best_d = usize::MAX;
        for y in 1..g.h - 1 {
            for x in 1..g.w - 1 {
                let i = y * g.w + x;
                if g.cost[i] == crate::godunov::BLOCKED {
                    continue;
                }
                let d = cx.abs_diff(x) + cy.abs_diff(y);
                if d < best_d {
                    best_d = d;
                    best = Some(i);
                }
            }
        }
        vec![best.expect("сцена целиком непроходима")]
    }

    /// **Порядок-независимость — свойство, а не заявление.**
    /// Шесть решателей и десять хаотических обходов обязаны дать
    /// побитово одно и то же поле.
    #[test]
    fn all_solvers_agree_bit_for_bit() {
        let h = h_exact_floor;
        for scene in Scene::ALL {
            for n in [16usize, 32, 64] {
                let g = build(scene, n, 12345);
                let goals = goals_center(&g);
                let (reference, _) = fmm(&g, &goals, &h);

                for (name, field) in [
                    ("gauss_seidel", gauss_seidel(&g, &goals, &h).0),
                    ("jacobi", jacobi(&g, &goals, &h).0),
                    ("fsm", fsm(&g, &goals, &h).0),
                    ("lsm", lsm(&g, &goals, &h).0),
                    ("fim", fim(&g, &goals, &h).0),
                ] {
                    assert_eq!(
                        field, reference,
                        "{name} разошёлся с эталоном: {} n={n}",
                        scene.name()
                    );
                }

                for seed in 0..5u64 {
                    for chunk in [1usize, 7, 64] {
                        let field = chaotic(&g, &goals, &h, seed, chunk);
                        assert_eq!(
                            field,
                            reference,
                            "хаотический обход seed={seed} chunk={chunk} разошёлся: {} n={n}",
                            scene.name()
                        );
                    }
                }
            }
        }
    }

    /// Порядок-независимость обязана держаться и при округлении к ближайшему,
    /// иначе амендмент к ADR-058 несовместим с ADR-058.
    #[test]
    fn order_independence_holds_for_round_to_nearest() {
        let h = crate::godunov::h_exact_round;
        for scene in [Scene::Surface, Scene::Tunnels, Scene::Road] {
            let g = build(scene, 48, 999);
            let goals = goals_center(&g);
            let (reference, _) = fmm(&g, &goals, &h);
            assert_eq!(lsm(&g, &goals, &h).0, reference, "{}", scene.name());
            for seed in 0..3u64 {
                assert_eq!(
                    chaotic(&g, &goals, &h, seed, 5),
                    reference,
                    "{} seed={seed}",
                    scene.name()
                );
            }
        }
    }

    /// Значение поля не должно зависеть от того, откуда начали обход,
    /// но обязано зависеть от цели. Тест ловит вырождение в константу.
    #[test]
    fn field_is_not_degenerate() {
        let g = build(Scene::Open, 32, 1);
        let goals = goals_center(&g);
        let (t, _) = fmm(&g, &goals, &h_exact_floor);
        assert_eq!(t[goals[0]], 0);
        assert!(max_finite(&t) > 100, "поле подозрительно плоское");
        assert_eq!(t.iter().filter(|&&v| v == INF).count(), g.w * g.h - g.inner_area());
    }
}
