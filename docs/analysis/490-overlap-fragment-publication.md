# 490 — Overlap fragment publication и slot alignment

3 ноября 2026, ADR-835. Артефакты: `out/optimization-490/`.

## Причина

`Batches::update` вычисляет resting groups/flags, а `slots.align` лишь выравнивает
render identities. Они выполнялись последовательно, хотя используют разные поля cache и
не читают результаты друг друга. `slots.prepare` зависит уже от обоих результатов.

## ADR-835

При `fragments.len() >= 2048` `Batches::update` и `slots.align` выполняются через
существующий ComputeTaskPool. `slots.prepare` начинается только после обоих и получает
прежние flags. Asset/entity commands и authoritative SIM не меняются; source order,
progressive publication и lifetime meshes сохранены.

## Проверки

`resting_batches_preserve_every_visible_piece_through_publication_wakeup_edits_and_reclaim`
прошёл, включая progressive publication, wakeup, compaction, reclaim, missing entity и
scene replacement.

## Native A/B

Четыре 60-секундных capture AB/BA против ADR-833 default compute8, Vulkan, 3840×2160,
Foreground+Maximized, saved tuning, navigation8/Rayon4. Все
`ValidForComparison=true`, `AcceptanceEligible=false`, поскольку 20 Гц нет.

| Пара | Mean FPS | p99 кадра | `fragments` p50 |
|---|---:|---:|---:|
|1, после→до|28,7→30,5 (+6,27%)|103,179→97,088 мс|4,554→3,790 мс|
|2, до→после|28,9→30,6 (+5,88%)|102,001→94,901 мс|4,444→3,957 мс|

Сводно FPS вырос примерно на **6,1%**, `fragments` p50 снизился примерно на **13%**,
p99 кадра — на **5,4%**. Realtime ratio вырос примерно с0,855 до0,895, но 20 Гц всё ещё
не достигнуто.

## Границы

Это устранение независимого render-барьера, а не ускорение SIM tick или retained-history
lifecycle. Новых buffers, persistent state и изменения gameplay нет.
