# 430 — Изменившиеся кучи через merge join по ID

1 ноября 2026, ADR-823. Артефакты: `out/optimization-430/`.

## Изменение

`Memo::changed` больше не строит большую hash-таблицу для нормального порядка
`FragmentId`. Предыдущий и текущий списки уже отсортированы по ID, поэтому
изменения вычисляются одним слиянием последовательностей. Нет `previous_by_id`
и массива `matched`. Несортированный импорт использует прежний hash fallback.

Изменённые кучи, полный merge, valid/no-transfers и формат снимков прежние.
Это ускорение инвалидации, а не решение всей retained-history нагрузки.

## Проверки

- `resting_join_memo_matches_cold_recomputation_after_geometry_and_support_changes`
  прошёл: тёплый и холодный пересчёт совпали после изменений геометрии и опоры.
- Новый тест `sorted_hash_and_unsorted_changes_match`: sorted, hash и
  несортированный путь возвращают одинаковые изменённые ключи.
- Во всех четырёх ABBA CSV совпали все36 нетайминговых полей каждого тика.
- Snapshot на тике300 и продолжение до600 дали `695327e0588d053c`.

## Измерения

Baseline `baseline-426.exe`, SHA-256
`ad8aa61502832211cc09d3ed01e68a59cb53ba6681669cc45058bdf609733bc9`.
World AI-vs-AI,1600 тиков,physics4/nav8/Rayon4.

| Показатель | Baseline | После | Изменение |
|---|---:|---:|---:|
|`merge_keys`, две пары|2,794→2,690 и2,674→2,595 мс|−3,7% и−3,0%|
|Полный mean|35,233 мс|35,139 мс|−0,27%|
|`debris`|16,525 мс|16,485 мс|−0,24%|
|Позднее окно1201–1600|51,505 мс|51,356 мс|−0,29%|

Результат небольшой, но направление совпало в обеих парах. Его нельзя суммировать
с другими срезами или выдавать за устранение главного retained-history bottleneck.

## Воспроизведение

```bash
RAYON_NUM_THREADS=4 ./out/optimization-430/baseline-426.exe \
  --world --ai-vs-ai --ticks 1600 --physics-workers 4 --workers 8 \
  --tick-csv out/optimization-430/before-1600-2.csv
RAYON_NUM_THREADS=4 ./out/optimization-425/source/target/release/bench_battle.exe \
  --world --ai-vs-ai --ticks 1600 --physics-workers 4 --workers 8 \
  --tick-csv out/optimization-430/after-1600-2.csv
```

P0-A продолжается: построение `current`, `Settled`, wake и merge groups остаются
следующими кандидатами после повторного профиля.
