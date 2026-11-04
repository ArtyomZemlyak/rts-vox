# 473 — Параллельная fragment render history: отклонено по FPS

3 ноября 2026. Артефакты: `out/optimization-473/`, не новый ADR.

## Гипотеза

`moving::before` последовательно преобразовывал позиции всей fragment history в
`(FragmentId, Vec3)`. Прототип заполнял тот же ordered Vec независимыми mutable
chunks через существующий ComputeTaskPool, без дополнительного collect или новых
allocations. Unordered import завершался прежней общей логикой; SIM не менялась.

`leaf_merge_matches_map_after_compaction_spawn_import_duplicates_and_reset` прошёл.

## Native методика

Четыре 60-секундных World AI-vs-AI capture, AB/BA. Vulkan, 3840×2160,
Foreground+Maximized, saved tuning, compute4/physics4/navigation8/Rayon4. Все capture
`ValidForComparison=true`, `AcceptanceEligible=false`, поскольку 20 Гц нет.

## Результаты

| Пара | Mean FPS | p99 кадра | `fragments` p50 |
|---|---:|---:|---:|
|1, после→до|29,3→29,2|104,950→105,996 мс|3,640→3,725 мс|
|2, до→после|29,6→29,3 (−1,01%)|105,735→105,858 мс|3,595→3,711 мс|

Первая пара дала небольшой плюс, обратная — регрессию. Сводно baseline29,4 FPS,
candidate29,3 FPS, около−0,34%; устойчивого улучшения полного кадра или `fragments`
нет. Реальный темп20 Гц по-прежнему не достигнут.

## Решение

Изменение отклонено и полностью откачено. Последовательный capture оказался не хуже
parallel mutable-chunk fill. Принятые ADR-828 и cumulative результаты не изменились.
