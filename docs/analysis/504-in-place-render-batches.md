# 504 — In-place reconciliation render batches

3 ноября 2026. Артефакты: `out/optimization-504/`.

## Причина

`Batches::update` при любом изменении извлекал весь `groups` в отдельный map и
вставлял каждую группу обратно. Это происходило даже для unaffected groups, чья
картинка и GPU mesh сохранялись. `fragments` остаётся самостоятельной native-стоимостью:
около5,5 мс в текущем профиле.

## Прототип

Новые tiles по-прежнему строятся в том же порядке. `groups` остаётся на месте:
`entry` обновляет members только у существующей группы, changed pictures проходят
прежний retire, `seen` и `retain` удаляют исчезнувшие ключи. `input/scratch`,
`flags`, progressive BUILDS, source indices и publication lifecycle не меняются.

## Проверки

`resting_batches_preserve_every_visible_piece_through_publication_wakeup_edits_and_reclaim`
прошёл, включая progressive publication, wakeup, compaction, reclaim, missing entity
и scene replacement.

Четыре 60-секундных capture AB/BA против ADR-838, Vulkan, 3840×2160,
Foreground+Maximized, saved tuning, navigation8/Rayon4. Все
`ValidForComparison=true`, `AcceptanceEligible=false`, поскольку 20 Гц нет.

| Пара | Mean FPS | p99 кадра | `fragments` p50 | `simulation_core` p50 |
|---|---:|---:|---:|---:|
|1, after→before|34,082→34,001|89,384→90,610 мс|2,967→3,032 мс|22,945→22,933 мс|
|2, before→after|33,801→34,099 (+0,88%)|90,484→90,618 мс|2,962→3,029 мс|22,838→22,752 мс|

Сводно FPS 33,901→34,090 (**+0,56%**), p99 кадра 90,547→90,001 мс
(**−0,60%**), `simulation_core` p50 **−0,16%**, realtime ratio 0,9239→0,9246.
`fragments` p50 практически не изменился (+0,03%): выигрыш относятся к полному
кадру, а не к обещанию ускорения каждой фазы.

## Решение

Изменение **принято как ADR-839**. In-place groups сохраняют прежний lifecycle и
устраняют полный map rebuild. Основная стоимость tiles/members и retained SIM
остаётся открытой.
