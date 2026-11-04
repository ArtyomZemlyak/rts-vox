# 512 — Fused motion routes и ground preparation

4 ноября 2026. Артефакты: `out/optimization-512/`.

## Причина

После ADR-841 native `motion` занимает около5,43 мс на тик. Routes и ground preparation
выполняются двумя последовательными Rayon-проходами по одним и тем же юнитам. Обе
стадии используют только свой `Motion` и неизменяемые world/rubble/snapshot blockers.

Простое параллельное выполнение routes и ground нельзя считать безопасным: они
разделяют per-unit caches. Поэтому прототип не запускает их друг против друга.

## Прототип

Один `par_chunks_mut(1)` проход выполняет для каждого актора прежнюю последовательность
`prepare_terrain_hunt` → optional local hunt → `prepare_ground`, затем возвращает оба
результата через `unzip`. Условия работы, search budgets, snapshot blockers и spatially
ordered commit прежние. Между акторами общего mutable state нет.

Устраняется отдельный pool barrier и повторный scheduling проход. Новый профильный
маркер `motion_routes_ground` обозначает объединённую стадию; старые маркеры в
bench/analyzers сохранены для прежних traces.

## Headless oracle и полные серии

200 тиков: все35 CSV-полей каждого тика и итоговый hash `0ad7faff4852df79` совпали
с ADR-838. Две1600-тиковые серии также сохранили все35 полей и hash
`ef96977a5b55489b`.

| Пара | Полный mean | Позднее окно1201–1600 |
|---|---:|---:|
|1, after→before|24,177→24,517 мс (−1,39%)|33,161→33,682 мс (−1,55%)|
|2, before→after|24,621→24,235 мс (−1,57%)|34,071→33,436 мс (−1,86%)|

Pooled: mean24,569→24,206 мс (**−1,48%**), позднее окно33,876→33,299 мс
(**−1,70%**), `motion`4,952→4,792 мс (**−3,23%**). Объединённый route+ground
учитывается в `route_us`, поэтому его рост не является регрессией отдельной работы.

Replay snapshot300→600 на physics1/4/8 дал `695327e0588d053c` во всех режимах.

## Native A/B

Четыре 60-секундных World AI-vs-AI capture AB/BA против ADR-841, Vulkan,
3840×2160, Foreground+Maximized, saved tuning, compute8/physics8/navigation8/Rayon4.
Все `ValidForComparison=true`, `AcceptanceEligible=false`, поскольку 20 Гц нет.

| Пара | Mean FPS | p99 кадра | `simulation_core` |
|---|---:|---:|---:|
|1, after→before|33,311→31,269 (+6,53%)|93,723→92,892 мс|24,686→25,176 мс|
|2, before→after|33,159→33,289 (+0,39%)|91,623→92,402 мс|24,956→24,712 мс|

Pooled FPS32,214→33,300 (**+3,37%**), `simulation_core` **−1,47%**,
`simulation_input` **−1,32%**, realtime ratio+0,20%. Средний p99 двух запусков
92,257→93,062 мс (**+0,87%**): хвост ухудшился в обеих парах и не объявляется
улучшенным. Абсолютные FPS ниже предыдущей серии у обоих вариантов, поэтому вывод
опирается на paired A/B, а не на сравнение разных окон.

## Решение

Изменение **принято как ADR-842** ради подтверждённого полного тика и среднего FPS,
с явно зафиксированной ценой p99. Это устранение scheduling-барьера, а не решение
retained-history или owned SIM publication.
