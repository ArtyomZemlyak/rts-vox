# 507 — HashSet для dirty-tile reconciliation

4 ноября 2026. Артефакты: `out/optimization-507/`.

## Причина и прототип

ADR-840 использует `BTreeSet` для `dirty` и `seen`, но эти контейнеры нужны только
для insert/contains; порядок не участвует в lifecycle. Прототип заменяет оба на
`bevy::platform::collections::HashSet`, сохраняя `BTreeMap` для tiles/groups, где
порядок значим. Это тот же membership, full rebuild и publication order.

## Проверки и результат

Lifecycle-тест прошёл. Четыре native capture AB/BA против ADR-840:

| Пара | Mean FPS | p99 кадра | `fragments` p50 | `simulation_core` p50 |
|---|---:|---:|---:|---:|
|1, hash→tree|34,874→35,454|87,329→87,364 мс|3,018→2,946 мс|23,011→22,779 мс|
|2, tree→hash|35,711→35,339 (−1,04%)|88,162→87,280 мс|2,845→2,913 мс|22,550→22,950 мс|

Сводно FPS 35,582→35,106 (**−1,34%**), p99 кадра−0,52%, `fragments`
**+2,42%**, `simulation_core` **+1,39%**, realtime ratio **−0,43%**.
Hash membership оказался дороже tree при фактическом размере и ключах dirty set.

## Решение

Прототип **отклонён и откачён**, `BTreeSet` и ADR-840 восстановлены. Наличие только
insert/contains ещё не означает, что hash будет быстрее дерева.
