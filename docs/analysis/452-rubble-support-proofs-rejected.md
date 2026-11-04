# 452 — Rubble support proofs: профиль и отклонение

2 ноября 2026. Артефакты: `out/optimization-452/`.

## Профиль

Diagnostic-only замер разделил rubble на classify, support и columns. Накопленные
средние к тику 1500:

- classify — 0,838 мс;
- support — 2,192 мс;
- columns — 0,311 мс;
- floor/stacked — 10 261/6 860 записей.

Лидером оказался второй `Settled` и support queries, а не построение колонок.
Profiling не менял gameplay: endpoint hash остался `ef96977a5b55489b`.

## Прототип

`RubbleSupportProof` сохранял stable ID и полные upright non-chunk входы query и
supporter. Reuse допускался только при совпадении обеих геометрий, resting supporter
и отсутствии текущего hull rider. Остальные записи использовали прежний builder.
`Settled::support_source` возвращал реального supporter из того же contact loop.
Floor собирался в исходном stacked order.

## Проверки

- `rubble_reused_boxes_match_reference_after_support_and_hull_changes` прошёл;
- 200 и 1600 тиков: все 35 нетайминговых полей каждого тика совпали;
- endpoint hash `ef96977a5b55489b` совпал.

## Производительность

| Показатель | ADR-825/826 | Support proofs |
|---|---:|---:|
|Mean 1600|30,041 мс|30,312 мс|
|Позднее окно1201–1600|42,531 мс|43,067 мс|
|`rubble`, mean|3,873 мс|4,069 мс|
|Support к тику1500|2,192 мс|2,400 мс|

Проверка stable IDs, полных входов и текущих hull riders оказалась дороже
сэкономленных rebuild. Общий тик и целевая фаза ухудшились.

## Решение

Прототип и profiling удалены, ADR-825/826 восстановлены. Restored release на 200
тиках дал 0 расхождений и mean 11,316 против 11,361 мс у baseline. Суммарный
принятый результат остаётся в [444](444-cumulative-825-measurement.md).
