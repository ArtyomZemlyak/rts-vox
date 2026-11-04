# 461 — Reuse capture `dented` buffer: отклонено

3 ноября 2026. Артефакты: `out/optimization-461/` и
`out/optimization-443/after-461.*`.

## Гипотеза

Каждый capture slice выделял `dented` на всю сетку. Derived buffer переиспользовал
capacity через `take/clear/resize`, сохраняя полное перезаполнение и прежний порядок.

## Проверки

- `parallel_slices_preserve_quota_frozen_poses_and_live_edits` прошёл.
- `a_sliced_capture_holds_a_moving_hull_in_its_frozen_pose` прошёл.
- Все181 nuclear hash совпали, ledger сбалансирован, hash `ae3798a582873796`.

## Производительность

| Показатель | ADR-827 | Reused `dented` |
|---|---:|---:|
|Mean tick|17,548 мс|41,110 мс|
|Худший тик|38,080 мс|114,389 мс|
|`read_ground`|173,537 мс|479,146 мс|
|claims|559,703 мс|803,352 мс|
|capture|22,562 мс|32,246 мс|

Все scopes и полный тик существенно ухудшились. Даже без учёта внешней вариативности
такой результат неприемлем.

## Решение

Прототип полностью откачен, обычный `vec![false; cells]` восстановлен. Restored
release на200 тиках дал 0 расхождений. Строгий cumulative остаётся в
[454](454-cumulative-827-measurement.md).
