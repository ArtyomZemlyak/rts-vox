# 511 — Overlap terrain publication и occupancy preparation

4 ноября 2026. Артефакты: `out/optimization-511/`.

## Причина

В native-профиле ADR-840 на кадрах с тиком `simulation_core` занимает 25,334 мс,
terrain/publication — 2,769 мс, occupancy — 2,475 мс. Последние две стадии идут
последовательно, хотя построение occupancy-кэша читает только SIM и `Visual`, а не
изменяемые `GasInput` words/dirty patches.

Это не полный owned SIM owner: такой owner требует отдельного протокола команд,
terrain deltas и событий. Текущий прототип устраняет конкретный доказанный барьер без
изменения авторитетной симуляции.

## Прототип

`prepare_occupancy` строит новый union и обновляет per-actor caches параллельно с
`apply_terrain` в существующем ComputeTaskPool. `finish_occupancy` затем выполняет
прежний ordered old/new merge, material resolve и dirty-word publication. Поэтому
terrain по-прежнему применяется до occupancy, а порядок слов и patches не меняется.

Для малых roster/отсутствующего pool сохраняется последовательный `occupancy()`;
другие вызовы и тесты используют его как обёртку над prepare/finish.

## Проверки

Release: 13 occupancy-тестов и отдельная gas-проверка прошли, ручной benchmark
пропущен как обычно. Новый split-vs-sequential oracle сравнивает words, dirty ranges,
column tops, revision, occupied cells и per-actor caches при движении/повреждении.
Lifecycle-тест resting fragments прошёл. Вложенный ComputeTaskPool проверен на1/4
workers существующим тестом.

Четыре 60-секундных World AI-vs-AI capture AB/BA, Vulkan, 3840×2160,
Foreground+Maximized, AutoNoVsync, saved tuning, compute8/physics8/navigation8/Rayon4.
Все `ValidForComparison=true`, `AcceptanceEligible=false`, поскольку 20 Гц нет.

| Пара | Mean FPS | p99 кадра |
|---|---:|---:|
|1, after→before|35,768→35,177 (+1,68%)|86,581→84,954 мс|
|2, before→after|35,521→35,719 (+0,55%)|86,647→85,406 мс|

Pooled интервалы: 35,349→35,743 FPS (**+1,12%**). Средний p99 двух запусков
85,800→85,994 мс (+0,23%); знаки пар разные, улучшение хвоста не заявляется.
На активном тике occupancy 2,436→0,888 мс (**−63,53%**), terrain/publication
2,731→2,494 мс (**−8,66%**), `simulation_input` −0,75%. `simulation_core`
изменился на+0,16%, то есть выигрыш относится к устранённому render-барьеру.

При одинаковых1150 тиках первая after/before пара дала одинаковый state hash
`885cbe2d18231f6c`. Realtime ratio остаётся около0,956, 20 Гц не достигнуто.

## Решение

Изменение **принято как ADR-841**. Независимая подготовка occupancy больше не ждёт
terrain publication; ordered commit и авторитетная симуляция не изменены. Это не
закрывает retained-history рост или полный owned SIM publication pipeline.
