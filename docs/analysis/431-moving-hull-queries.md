# 431 — Moving-only запросы корпусов в wake

1 ноября 2026, ADR-824. Артефакты: `out/optimization-431/`.

## Изменение

Bulldoze/push возможен только при ненулевой скорости корпуса, но wake вызывал
общий `HullQueries::hit` и проверял также неподвижные корпуса. Новый
`hit_moving` оставляет только moving hulls. BVH-узлы содержат флаг наличия
движущегося корпуса, поэтому неподвижные ветки пропускаются без проверок.
Scalar fallback выполняет прежний exact contact только для moving hulls.

Первый контакт в canonical source order, skip masks, geometry и правила push
не меняются. Общий `hit` сохраняет прежнее поведение.

## Проверки

- Новый `moving_contacts_match_filtered_scalar_reference`: смешанные скорости,
  0–96 корпусов, повороты, дыры и skip masks совпали с независимым фильтром.
- Существующий `indexed_contacts_match_source_order...` прошёл.
- Все36 нетайминговых полей каждого тика обеих ABBA серий совпали.
- Snapshot на тике300 и продолжение до600 дали `695327e0588d053c`.

## ABBA

Baseline `baseline-430.exe`, SHA-256
`05521f4bcc15cab684c706238a2aba0bd59ce5ac655b25ea46d34a9c39569081`.
World AI-vs-AI,1600 тиков,physics4/nav8/Rayon4.

| Пара | Полный mean | Позднее окно1201–1600 |
|---|---:|---:|
|before-1 / after-1|35,688→34,549 мс (−3,19%)|52,122→50,517 мс (−3,08%)|
|before-2 / after-2|34,970→35,187 мс (+0,62%)|51,152→51,441 мс (+0,57%)|

Среднее двух пар: mean35,329→34,868 мс (−1,31%), позднее окно−1,27%.
Wake в первой паре6,508→6,324 мс, во второй6,335→6,441 мс, поэтому отдельное
устойчивое ускорение wake не заявляется. Подтверждённый результат — улучшение
полного тика в обеих сериях, а не отдельный процент каждой подфазы.

## Воспроизведение

```bash
RAYON_NUM_THREADS=4 ./out/optimization-431/baseline-430.exe \
  --world --ai-vs-ai --ticks 1600 --physics-workers 4 --workers 8 \
  --tick-csv out/optimization-431/before-1600-2.csv
RAYON_NUM_THREADS=4 ./out/optimization-431/source/target/release/bench_battle.exe \
  --world --ai-vs-ai --ticks 1600 --physics-workers 4 --workers 8 \
  --tick-csv out/optimization-431/after-1600-2.csv
```

P0-A продолжается: `Settled`, merge groups и построение `current` остаются
крупными потребителями retained history.
