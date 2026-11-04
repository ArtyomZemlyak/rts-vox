# 438 — Persistent world-boxes `Settled`: отклонено

1 ноября 2026. Артефакты: `out/optimization-438/`.

## Гипотеза

`Settled` вычисляет world-box каждого фрагмента на каждом тике. Прототип сохранял
boxes между тиками по stable ID и точным входам: position, basis, shape, side,
mm и chunk bounds. Совпадение позволяло пропустить прежний `Fragment::bounds`.

Это был первый узкий срез persistent `Settled`, не persistent cell membership.

## Корректность и граница кэша

Первый lab restore выявил, что общий builder также вызывается rubble с внешними
local-boxes: их изменение не описывается только полями фрагмента. Reuse был
ограничен основной debris-фазой с `f.bounds()`; rubble и generic test wrapper
сохранили полный пересчёт.

После этого прошли:

- `lab_fragment_landing_he_shell_retains_finite_shapes_and_restores_the_settled_world`;
- `settled_rebuilds_preserve_sweeps_and_clear_empty_generations`;
- release-сборка;
- все 36 нетайминговых полей каждого тика A/B и hash `ef96977a5b55489b`.

Известный baseline `zero_room...` не использовался как доказательство регрессии.

## Измерения

Baseline `baseline-437.exe` после удаления отклонённого selected-scratch,
SHA-256 `df455eaa34243cfb006f6a3ae8da9542b925d19b1a1e6776a0b26b9ca807388b`.
World AI-vs-AI, 1600 тиков, physics 4/navigation 8/Rayon 4, порядок «после/до».

| Показатель | Baseline | После | Изменение |
|---|---:|---:|---:|
|Полный mean|32,554 мс|33,062 мс|+1,56%|
|Позднее окно1201–1600|46,233 мс|47,595 мс|+2,94%|
|`debris`|13,831 мс|13,858 мс|+0,20%|
|`settled_boxes`, позднее окно|1,198 мс|1,374 мс|+14,70%|

Точное сравнение входов, чтение/запись дополнительных `box_ids/box_inputs` и
параллельная упаковка трёх массивов стоят дороже повторного вычисления boxes.
Обратная серия не оправдана отрицательным полным и целевым результатом.

## Решение

Прототип не переносится в рабочее дерево, ADR не создаётся. Persistent `Settled`
не отвергается как архитектура: отвергнут именно signature-cache boxes. Следующий
большой срез должен устранять полные проходы/membership и иметь измеренный dirty
журнал, а не добавлять сравнение всех полей каждого фрагмента на каждом тике.
