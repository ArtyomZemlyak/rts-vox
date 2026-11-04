# 450 — Transition-only batch join: точность есть, общий выигрыш отсутствует

2 ноября 2026. Артефакты: `out/optimization-450/`.

## Гипотеза и отличие от 440

После ADR-825 полный `Memo::current` и previous/current diff остаются O(retained).
Прототип журналил только join-eligible transitions (emit/landing/wake/merge), а не
все flying IDs. Dirty updates применялись одной sorted batch-операцией, без per-ID
insertion в середину `Vec`. Canonical source slots восстанавливались линейным reindex.

Reclaim/sweep и unsorted input использовали full rebuild. Test-only reconcile строил
холодный полный вход и заменял batch результат полным при любом расхождении, включая
прямые setup/test-мутации `fragments`.

## Корректность

Прошли:

- `batch_updates_match_the_cold_diff` для update/insert/remove;
- `resting_join_memo_matches_cold_recomputation_after_geometry_and_support_changes`;
- `the_world_battle_hash_does_not_depend_on_physics_workers` на physics 1/2/4/8;
- release World AI-vs-AI 1600 тиков, physics4/navigation8/Rayon4: все 35
  нетайминговых CSV-полей каждого тика совпали;
- endpoint hash `ef96977a5b55489b` совпал.

## Производительность

Первая версия консервативно делала full rebuild в каждом `range_fracture` batch.
Это вернуло дорогую полную группировку: merge groups в позднем окне выросли примерно
с 0,52 до 2,35 мс, полный mean составил 31,285 против 30,041 мс.

После замены fallback на точные stable-ID marks changed heads/donors:

| Показатель | ADR-825/826 | Batch | Изменение |
|---|---:|---:|---:|
|Mean 1600 |30,041 мс|30,178 мс|+0,45% (медленнее)|
|Позднее окно1201–1600|42,531 мс|42,687 мс|+0,37% (медленнее)|
|`debris`, полный бой|12,626 мс|12,713 мс|+0,69%|
|`merge_keys`, 1501–1600|3,067 мс|2,618 мс|−14,64%|

Целевая фаза стала дешевле, но полный тик не улучшился. Копирование current,
canonical reindex, dirty journal и guards поглотили выигрыш. Как и в 439/440,
алгоритмически меньшая работа не дала принимаемого ускорения.

## Решение и восстановление

Прототип отклонён. `join_dirty`, batch memo, fracture marks и test reconcile удалены;
рабочее дерево возвращено к ADR-825/826. Restored release на 200 тиках:

- 0 расхождений по 35 нетайминговым полям;
- mean 11,361 мс против 11,390 мс у контрольного baseline.

Суммарный принятый результат остаётся описанным в
[444](444-cumulative-825-measurement.md). Следующий крупный retained consumer — wake,
а не дальнейшие перестановки полного join membership.
