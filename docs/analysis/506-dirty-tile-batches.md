# 506 — Dirty-tile reconciliation render batches

3 ноября 2026. Артефакты: `out/optimization-506/`.

## Причина и прототип

После ADR-839 группы обновляются на месте, но все tiles и их members всё равно
пересобираются при любом изменении. Прототип отмечает dirty tile по старому и новому
положению для:

- изменённой геометрии или source index;
- потери resting/zero-velocity eligibility;
- перемещения между tiles или смены материала;
- ожидания landing interpolation.

Unaffected groups сохраняются без перестройки. `seen` заранее содержит их ключи;
полная scene generation смена использует прежний полный rebuild. `input/scratch`,
source order, progressive BUILDS и publication lifecycle не меняются.

Это инкрементальная render reconciliation, а не persistent SIM index или повторение
inline change detection 447.

## Проверки

Первый lifecycle запуск нашёл два краевых случая, исправленных до замера:
`!enabled` и пустой `scratch` вызывают полный rebuild/retire. После исправлений
`resting_batches_preserve_every_visible_piece_through_publication_wakeup_edits_and_reclaim`
прошёл, включая progressive publication, wakeup, compaction, reclaim и scene replacement.

Четыре 60-секундных capture AB/BA против ADR-839, Vulkan, 3840×2160,
Foreground+Maximized, saved tuning, navigation8/Rayon4. Все
`ValidForComparison=true`, `AcceptanceEligible=false`, поскольку 20 Гц нет.

| Пара | Mean FPS | p99 кадра | `fragments` p50 | Realtime ratio |
|---|---:|---:|---:|---:|
|1, after→before|35,337→34,127|85,681→90,088 мс|2,888→2,961 мс|0,9567→0,9269|
|2, before→after|34,048→35,479 (+4,20%)|89,568→86,878 мс|2,995→2,970 мс|0,9257→0,9543|

Сводно FPS 34,088→35,408 (**+3,87%**), p99 кадра 89,828→86,279 мс
(**−3,95%**), `fragments` p50 **−1,64%**, realtime ratio 0,9263→0,9555
(**+3,15%**). `simulation_core` p50 изменился на+0,28%, поэтому эффект относится
к render reconciliation, а не к SIM.

## Решение

Изменение **принято как ADR-840** (4 ноября). Unaffected tiles больше не
пересобираются при каждом изменении. Полный rebuild сохранён для scene generation,
disabled batching и пустого scratch.
