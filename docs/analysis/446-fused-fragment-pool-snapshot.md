# 446 — Pool snapshot в существующем обходе фрагментов

2 ноября 2026, ADR-826. Артефакты: `out/optimization-446/`.

## Изменение

`step_fragments_kernel` раньше делал полный `keep_on_map`, затем после `Settled`
вызывал `fragment_pool_snapshot` и снова обходил всю историю. `Settled` не меняет
фрагменты, а `parkable`, `resting` и `chunk` не меняются от clamp позиции.

Теперь flying/parked/parked_end вычисляются в том же исходном обходе `keep_on_map`.
Повторный pool snapshot удалён. Сами clamp, классификация, порядок parked_end,
квоты, wake, контакты и формат снимков прежние.

## Проверки

- `sleeping_history_keeps_its_matter_and_does_not_block_new_claims` прошёл.
- `damage_waiting_keeps_the_missed_deadline_support_in_the_index` прошёл на1/4/8.
- Lab fragment landing/snapshot restore прошёл.
- В обеих1600-сериях совпали все36 нетайминговых полей каждого тика.
- Snapshot на тике300 и продолжение до600 на physics1/4/8 дали одинаковый
  `695327e0588d053c`; replay и итоговые хеши совпали.

## Измерения

Baseline `out/optimization-444/current-825.exe`, SHA-256
`3b2efc1749d767ab0ae7ce579a96ac3bddf05257978d4ea79852e4f39c928b53`.
World AI-vs-AI,1600 тиков,physics4/navigation8/Rayon4.

| Пара | Полный mean | Позднее окно1201–1600 |
|---|---:|---:|
|1, после/до|30,484→30,417 мс (−0,22%)|43,339→42,867 мс (−1,09%)|
|2, до/после|30,518→30,417 мс (−0,33%)|43,165→42,912 мс (−0,59%)|

Сводно:

| Показатель | Baseline | После | Изменение |
|---|---:|---:|---:|
|Полный mean|30,501 мс|30,417 мс|−0,27%|
|Позднее окно|43,252 мс|42,890 мс|−0,84%|
|`debris`|12,846 мс|12,739 мс|−0,84%|

Выигрыш небольшой, но повторился в обеих сериях. Это удаление одного линейного
прохода, а не решение retained-history нагрузки и не доказанный native FPS gain.

## Воспроизведение

```bash
RAYON_NUM_THREADS=4 ./target/release/bench_battle.exe \
  --world --ai-vs-ai --ticks 1600 --physics-workers 4 --workers 8 \
  --tick-csv out/optimization-446/after-1600-1.csv
RAYON_NUM_THREADS=4 ./out/optimization-444/current-825.exe \
  --world --ai-vs-ai --ticks 1600 --physics-workers 4 --workers 8 \
  --tick-csv out/optimization-446/before-1600-1.csv
```
