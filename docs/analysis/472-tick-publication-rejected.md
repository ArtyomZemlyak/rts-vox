# 472 — Пропуск повторной публикации неизменного tick: отклонено по FPS

3 ноября 2026. Артефакты: `out/optimization-472/`, не новый ADR.

## Гипотеза

`Batches::update` сканировал resting fragments на каждом render frame, включая кадры
без нового SIM tick. Прототип пропускал повтор только при совпадении
`(generation, tick, fragments.len, paused, enabled)` и после полной публикации всех
groups с существующими ECS entities.

Первый lifecycle-запуск выявил преждевременный skip при pending meshes. Условие было
исправлено: skip разрешён только после завершения публикации. После этого тест
`resting_batches_preserve_every_visible_piece_through_publication_wakeup_edits_and_reclaim`
прошёл, включая progressive publication, wakeup, compaction, reclaim, missing entity
и scene replacement.

## Native методика

Четыре 60-секундных World AI-vs-AI capture, AB/BA. Vulkan, 3840×2160,
Foreground+Maximized, AutoNoVsync, saved tuning, compute4/physics4/navigation8/Rayon4.
Все четыре runner-результата: `ValidForComparison=true`, `AcceptanceEligible=false`,
потому что 20 Гц не достигнуто.

Baseline — `out/optimization-457/current-827-blast.exe`; candidate —
`tick-publication-blast.exe` с ADR-828 в SIM и publication-ключом.

## Результаты

| Пара | Mean FPS | p99 кадра | `fragments` p50 |
|---|---:|---:|---:|
|1, после→до|29,4→29,2 (+0,68% у baseline-повтора)|106,496→107,197 мс|3,557→3,819 мс|
|2, до→после|29,2→29,2|106,167→107,018 мс|3,591→3,809 мс|

В первой паре candidate был немного быстрее, во второй разницы нет. Сводно разница
около +0,3% FPS при исходных 29,2 FPS, без повторяемого улучшения `fragments` или p99.
В перегруженном бою почти каждый кадр содержит новый SIM tick, поэтому repeat updates
редко исключаются. Realtime ratio остаётся около0,84, 20 Гц не достигнуто.

## Решение

Изменение отклонено и полностью откачено. Корректность механизма подтверждена, но
практической экономии в целевом сценарии нет. Принятые ADR-828 и cumulative headless
результаты не изменились. Для OPT316-08 нужен owned pipeline, который устраняет работу
в каждом тике или блокировку кадра, а не только повтор кадров без нового tick.
