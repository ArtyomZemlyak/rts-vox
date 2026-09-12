// Щелевое затенение по вокселям: постпроцесс по глубине и нормалям
// (ADR-235).
//
// **Самодостаточный**: ни одного `#import` из модулей Bevy и ни одного
// shader def препроцессора (`CLAUDE.md`). Знакового `%` здесь нет.
//
// Зачем свой, когда у Bevy есть щелевое затенение. У него радиус зашит
// в шейдере — 0.5·1.457 мировой единицы, то есть семь сантиметров при
// ребре мира 10 см, — и расширить его без shader defs нельзя. Воронка,
// подножие обрыва и щель между машиной и землёй лежат на масштабе
// в несколько вокселей, и до них оно не достаёт.
//
// Вход — экран **до** тонмаппинга (линейный свет), глубина и нормали
// из префасса. Выход — тот же экран, умноженный на затенение.
// Умножается весь свет, а не только рассеянный: разделить их после
// расчёта нельзя, и это честная цена за то, что проход не залезает
// в PBR Bevy.
//
// Шум. У каждого пикселя свой поворот набора выборок, иначе набор
// виден полосами; ценой становится зерно. Отдельного прохода
// размытия нет — вместо него затенение усредняется по квадрату 2×2
// **производными**: `dpdxFine`/`dpdyFine` дают разность с соседом
// по строке и по столбцу того же квадрата, и три значения из четырёх
// приходят даром. Ради этого в функции нет ранних выходов: производная
// обязана считаться в однородном потоке управления.

struct Occlusion {
    view_from_clip: mat4x4<f32>,
    clip_from_view: mat4x4<f32>,
    view_from_world: mat4x4<f32>,
    // Радиус полусферы выборок, мировые единицы.
    radius: f32,
    // Сила затемнения, 0..1.
    strength: f32,
    // Смещение по глубине против самозатенения, мировые единицы.
    bias: f32,
    // Степень кривой: больше — тень собирается в самой щели.
    power: f32,
};

@group(0) @binding(0) var screen_tex: texture_2d<f32>;
@group(0) @binding(1) var screen_smp: sampler;
@group(0) @binding(2) var depth_tex: texture_depth_2d;
@group(0) @binding(3) var normal_tex: texture_2d<f32>;
@group(0) @binding(4) var<uniform> cfg: Occlusion;

// Выход полноэкранной вершинной программы Bevy: тот же набор мест,
// но объявленный у себя, а не притянутый импортом.
struct In {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

const SAMPLES: i32 = 16;

fn depth_at(p: vec2<i32>, hi: vec2<i32>) -> f32 {
    return textureLoad(depth_tex, clamp(p, vec2<i32>(0, 0), hi), 0);
}

// Точка в пространстве вида по координате экрана и глубине. UV идёт
// сверху вниз, клип — снизу вверх, отсюда переворот по `y`.
fn view_pos(uv: vec2<f32>, depth: f32) -> vec3<f32> {
    let ndc = vec3<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, depth);
    let p = cfg.view_from_clip * vec4<f32>(ndc, 1.0);
    return p.xyz / p.w;
}

// Чередующийся градиентный шум: у каждого пикселя свой поворот
// набора выборок, и полосы от общего набора рассыпаются в зерно.
fn noise(p: vec2<f32>) -> f32 {
    return fract(52.9829189 * fract(0.06711056 * p.x + 0.00583715 * p.y));
}

// Доля перекрытых выборок в пикселе, 0..1. Небо — минус единица:
// его не затеняют и соседям по квадрату оно не помогает.
fn occlusion_at(pos_px: vec2<f32>, uv: vec2<f32>) -> f32 {
    let dims = vec2<i32>(textureDimensions(screen_tex));
    let hi = dims - vec2<i32>(1, 1);
    let p = vec2<i32>(floor(pos_px));
    let d0 = depth_at(p, hi);
    // Глубина обратная: ноль — это небо, затенять там нечего.
    if (d0 <= 0.0) {
        return -1.0;
    }
    let pos = view_pos(uv, d0);
    let nw = textureLoad(normal_tex, clamp(p, vec2<i32>(0, 0), hi), 0).xyz * 2.0 - vec3<f32>(1.0, 1.0, 1.0);
    let n = normalize((cfg.view_from_world * vec4<f32>(nw, 0.0)).xyz);

    // Полусфера выборок в касательном базисе нормали, повёрнутом
    // случайно вокруг неё.
    let a = noise(pos_px) * 6.2831853;
    let rv = vec3<f32>(cos(a), sin(a), 0.0);
    var t = rv - n * dot(rv, n);
    if (dot(t, t) < 1e-4) {
        t = vec3<f32>(0.0, 0.0, 1.0) - n * n.z;
    }
    t = normalize(t);
    let b = cross(n, t);

    // Шестнадцать точек: гуще у центра, реже к краю полусферы.
    var kernel = array<vec3<f32>, 16>(
        vec3<f32>(-0.1133, 0.0109, 0.1027),
        vec3<f32>(0.1134, 0.0966, 0.0670),
        vec3<f32>(-0.0152, 0.1240, 0.1294),
        vec3<f32>(-0.0600, -0.0867, 0.1736),
        vec3<f32>(-0.1126, -0.1795, 0.0969),
        vec3<f32>(-0.0672, -0.1522, 0.2121),
        vec3<f32>(0.1088, 0.1745, 0.2356),
        vec3<f32>(-0.2099, -0.0906, 0.2814),
        vec3<f32>(-0.0353, -0.2397, 0.3418),
        vec3<f32>(0.3027, -0.1551, 0.3416),
        vec3<f32>(0.1674, -0.0812, 0.5195),
        vec3<f32>(-0.1171, 0.3699, 0.4940),
        vec3<f32>(-0.1232, -0.5591, 0.4219),
        vec3<f32>(-0.0594, -0.7105, 0.3647),
        vec3<f32>(-0.6934, -0.5016, 0.2688),
        vec3<f32>(-0.3644, -0.7533, 0.5475),
    );

    let dimsf = vec2<f32>(dims);
    var occ = 0.0;
    for (var i = 0; i < SAMPLES; i = i + 1) {
        let k = kernel[i];
        let s = pos + (t * k.x + b * k.y + n * k.z) * cfg.radius;
        let c = cfg.clip_from_view * vec4<f32>(s, 1.0);
        if (c.w <= 1e-5) {
            continue;
        }
        let ndc = c.xy / c.w;
        let suv = vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
        let sp = vec2<i32>(floor(suv * dimsf));
        if (sp.x < 0 || sp.y < 0 || sp.x > hi.x || sp.y > hi.y) {
            continue;
        }
        let ds = depth_at(sp, hi);
        if (ds <= 0.0) {
            continue;
        }
        let scene_z = view_pos(suv, ds).z;
        // Ось вида смотрит в −Z: сцена ближе камеры, чем точка выборки,
        // когда её `z` больше. Далёкая стенка не затеняет: вклад
        // гаснет с расстоянием между пикселем и тем, что закрыло точку.
        if (scene_z - s.z > cfg.bias) {
            occ = occ + smoothstep(0.0, 1.0, cfg.radius / abs(pos.z - scene_z));
        }
    }
    return occ / f32(SAMPLES);
}

@fragment
fn fragment(in: In) -> @location(0) vec4<f32> {
    let colour = textureSample(screen_tex, screen_smp, in.uv);
    let own = occlusion_at(in.position.xy, in.uv);

    // Соседи по квадрату 2×2: производная — разность с соседом,
    // и знак зависит от того, левый пиксель или правый, верхний
    // или нижний. Соседа по диагонали производные не дают, и его
    // место занимает среднее двух.
    let dx = dpdxFine(own);
    let dy = dpdyFine(own);
    let px = vec2<u32>(floor(in.position.xy));
    let sx = select(-1.0, 1.0, (px.x & 1u) == 0u);
    let sy = select(-1.0, 1.0, (px.y & 1u) == 0u);
    let right = clamp(own + sx * dx, 0.0, 1.0);
    let below = clamp(own + sy * dy, 0.0, 1.0);
    let avg = (2.0 * max(own, 0.0) + right + below) * 0.25;
    // Небо не затеняется и от соседей по квадрату, иначе по краю
    // силуэта на небе шла бы тёмная кайма в пиксель.
    let occ = select(avg, 0.0, own < 0.0);

    let ao = clamp(1.0 - occ, 0.0, 1.0);
    let k = mix(1.0, pow(ao, cfg.power), cfg.strength);
    return vec4<f32>(colour.rgb * k, colour.a);
}
