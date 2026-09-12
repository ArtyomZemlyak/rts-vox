// Обводка силуэта по глубине и нормалям плюс ступени яркости (ADR-214).
//
// **Самодостаточный**: ни одного `#import` из модулей Bevy и ни одного
// shader def препроцессора (`CLAUDE.md`). Знакового `%` здесь нет —
// он запрещён целиком.
//
// Вход — экран **после** тонмаппинга, глубина и нормали из префасса.
// Обводка по яркости была бы дешевле и неверна: она ставит линию
// на границе цвета, а граница цвета — это не силуэт.

struct Edges {
    // Цвет линии, **линейный**: кадр здесь ещё не закодирован в sRGB.
    colour: vec4<f32>,
    // Толщина линии в пикселях: шаг до соседа, с которым сравниваем.
    width: f32,
    // Порог по глубине: относительная разница, а не абсолютная.
    depth: f32,
    // Порог по нормали: единица минус косинус угла между соседями.
    normal: f32,
    // Сила линии, 0..1.
    strength: f32,
    // Сколько ступеней яркости. Меньше двух — ступеней нет.
    steps: f32,
    // Насколько ступени подмешиваются к исходному цвету, 0..1.
    posterise: f32,
    pad0: f32,
    pad1: f32,
};

@group(0) @binding(0) var screen_tex: texture_2d<f32>;
@group(0) @binding(1) var screen_smp: sampler;
@group(0) @binding(2) var depth_tex: texture_depth_2d;
@group(0) @binding(3) var normal_tex: texture_2d<f32>;
@group(0) @binding(4) var<uniform> cfg: Edges;

// Выход полноэкранной вершинной программы Bevy: тот же набор мест,
// но объявленный у себя, а не притянутый импортом.
struct In {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

fn depth_at(p: vec2<i32>, hi: vec2<i32>) -> f32 {
    return textureLoad(depth_tex, clamp(p, vec2<i32>(0, 0), hi), 0);
}

fn normal_at(p: vec2<i32>, hi: vec2<i32>) -> vec3<f32> {
    let n = textureLoad(normal_tex, clamp(p, vec2<i32>(0, 0), hi), 0).xyz;
    return normalize(n * 2.0 - vec3<f32>(1.0, 1.0, 1.0));
}

@fragment
fn fragment(in: In) -> @location(0) vec4<f32> {
    var colour = textureSample(screen_tex, screen_smp, in.uv);

    let dims = vec2<i32>(textureDimensions(screen_tex));
    let hi = dims - vec2<i32>(1, 1);
    let p = vec2<i32>(floor(in.position.xy));
    let w = max(i32(cfg.width), 1);

    let d0 = depth_at(p, hi);
    let n0 = normal_at(p, hi);

    // Четыре соседа крестом. Диагонали на воксельной сцене ничего
    // не добавляют — рёбра здесь осевые, — а стоят вдвое дороже.
    var offs = array<vec2<i32>, 4>(
        vec2<i32>(w, 0),
        vec2<i32>(-w, 0),
        vec2<i32>(0, w),
        vec2<i32>(0, -w),
    );
    var dmax = 0.0;
    var nmax = 0.0;
    for (var i = 0; i < 4; i = i + 1) {
        let q = p + offs[i];
        let d = depth_at(q, hi);
        // Глубина обратная: ближе — больше. Линия ставится **только
        // на ближнем** из двух, иначе силуэт обводится дважды и выходит
        // вдвое толще обещанного. Разница относительная: она не зависит
        // ни от дальности, ни от проекции.
        dmax = max(dmax, (d0 - d) / max(d0, 1e-6));
        let n = normal_at(q, hi);
        nmax = max(nmax, 1.0 - dot(n, n0));
    }

    // Не `step`: ровно ступенька даёт лесенку на наклонной линии.
    // Полоса узкая — линия остаётся линией, а не размазнёй.
    let ed = smoothstep(cfg.depth, cfg.depth * 1.6 + 1e-6, dmax);
    let en = smoothstep(cfg.normal, cfg.normal * 1.6 + 1e-6, nmax);
    let e = clamp(max(ed, en) * cfg.strength, 0.0, 1.0);

    // Ступени яркости. Считаются **в перцептивной шкале**: ровные
    // ступени в линейном свете сгущаются в тенях и расходятся
    // в светах, то есть ступенями не выглядят.
    if (cfg.steps >= 2.0) {
        let l = max(dot(colour.rgb, vec3<f32>(0.2126, 0.7152, 0.0722)), 1e-5);
        let g = pow(l, 1.0 / 2.2);
        let q = clamp(floor(g * cfg.steps + 0.5) / cfg.steps, 0.0, 1.0);
        let lq = pow(q, 2.2);
        // Множителем, а не подменой: оттенок остаётся своим,
        // ступенями идёт только светлота.
        colour = vec4<f32>(mix(colour.rgb, colour.rgb * (lq / l), cfg.posterise), colour.a);
    }

    return vec4<f32>(mix(colour.rgb, cfg.colour.rgb, e), colour.a);
}
