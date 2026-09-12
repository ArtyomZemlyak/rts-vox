//! Состав поверхности: карта не бывает одного цвета.
//!
//! Проверка заведена не из любви к числам. Генератор дважды приводил
//! к карте, где половину кадра занимал один материал: сначала ровным
//! слоем почвы поверх всего (ADR-210), потом — почвой в лощинах
//! и на гребнях одинаково (ADR-217). Оба раза это было видно только
//! на снимке, то есть слишком поздно.

use worldgen::mat::{self, Mat};
use worldgen::{material, soil_depth, surface_height, Params};

/// Карта показа: её и смотрит владелец.
fn show_map() -> Params {
    Params { relief: 30, ..Params::sized(384, 192, 384) }
}

/// Доли материалов на самой поверхности, в процентах по номеру.
fn surface_mix(p: &Params, seed: u64) -> ([f64; 64], u32) {
    let mut hist = [0u32; 64];
    let mut n = 0u32;
    // Через воксель: шаг вдвое мельче ничего не меняет в долях,
    // а прогон вчетверо дольше.
    for z in (0..p.nz).step_by(2) {
        for x in (0..p.nx).step_by(2) {
            let h = surface_height(p, seed, x, z);
            let m: Mat = material(p, seed, x, h - 1, z);
            hist[(m as usize) & 63] += 1;
            n += 1;
        }
    }
    let mut out = [0.0f64; 64];
    for (o, c) in out.iter_mut().zip(hist.iter()) {
        *o = *c as f64 * 100.0 / n as f64;
    }
    (out, n)
}

/// **Ни один материал не занимает поверхность целиком.**
///
/// Число замерено на карте показа и оставлено с запасом: порог ловит
/// возврат к «одному цвету», а не караулит текущее значение.
#[test]
fn no_single_material_owns_the_surface() {
    let p = show_map();
    let (mix, n) = surface_mix(&p, 42);
    assert!(n > 30_000, "проверка ничего не проверила: клеток {n}");
    let mut top = 0usize;
    for (i, v) in mix.iter().enumerate() {
        if *v > mix[top] {
            top = i;
        }
    }
    assert!(
        mix[top] <= 65.0,
        "материал {top} занимает {:.1}% поверхности — карта одного цвета",
        mix[top]
    );
}

/// **Порода выходит наружу, а руду видно, где копать.**
///
/// Без этого карта проходит проверку выше и всё равно остаётся ровным
/// полем: два оттенка бурого делят её пополам.
#[test]
fn rock_and_ore_reach_the_surface() {
    let p = show_map();
    let (mix, _) = surface_mix(&p, 42);
    let rock = mix[mat::ROCK as usize] + mix[mat::DEEP_ROCK as usize];
    let ore: f64 = [
        mat::IRON_POOR,
        mat::IRON,
        mat::IRON_RICH,
        mat::COPPER_POOR,
        mat::COPPER,
        mat::COPPER_RICH,
    ]
    .iter()
    .map(|m| mix[*m as usize])
    .sum();
    assert!(rock >= 30.0, "голой породы всего {rock:.1}%");
    assert!(ore >= 2.0, "руды на поверхности всего {ore:.1}%");
}

/// **Высота над базой снимает почву.**
///
/// Правило ADR-217 проверяется прямо: на гребне слой обязан быть
/// тоньше, чем в лощине. Сравниваются средние, а не отдельные клетки:
/// пятно шума на одной клетке сильнее превышения.
#[test]
fn ridges_lose_their_soil() {
    let p = show_map();
    let (mut high, mut low) = ((0u64, 0u32), (0u64, 0u32));
    for z in (0..p.nz).step_by(2) {
        for x in (0..p.nx).step_by(2) {
            let h = surface_height(&p, 42, x, z);
            let d = soil_depth(&p, 42, x, z) as u64;
            if h >= p.base_height + 15 {
                high = (high.0 + d, high.1 + 1);
            } else if h + 10 <= p.base_height {
                low = (low.0 + d, low.1 + 1);
            }
        }
    }
    assert!(high.1 > 500 && low.1 > 500, "клеток мало: гребней {}, лощин {}", high.1, low.1);
    let (a, b) = (high.0 as f64 / high.1 as f64, low.0 as f64 / low.1 as f64);
    assert!(a < b * 0.6, "на гребне почвы {a:.2}, в лощине {b:.2} — разницы нет");
}

/// **Карта эталонного прогона перемешана так же.**
///
/// На ней стоит golden-хеш, и если правило залегания её не задевает,
/// то хеш и не заметит, как генератор переложит материалы по всему
/// миру (ADR-116 про то, что сценарий обязан нагружать то, что
/// декларирует).
#[test]
fn the_reference_map_is_mixed_too() {
    let p = Params::sized(256, 128, 256);
    let (mix, _) = surface_mix(&p, 42);
    let soil = mix[mat::SOIL as usize];
    let rock = mix[mat::ROCK as usize] + mix[mat::DEEP_ROCK as usize];
    println!("эталон: почва {soil:.1}%, порода {rock:.1}%");
    assert!(soil <= 65.0, "почвы {soil:.1}% — эталонная карта одного цвета");
    assert!(rock >= 25.0, "голой породы {rock:.1}%");
}
