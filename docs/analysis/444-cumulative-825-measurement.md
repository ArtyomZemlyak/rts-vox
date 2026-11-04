# 444 — Суммарный замер ADR-825 против baseline до ADR-819

2 ноября 2026. Артефакты: `out/optimization-444/`.

## Бинарники

- Baseline до ADR-819: `out/optimization-401/baseline/bench_battle-before.exe`,
  SHA-256 `dfec8120c4eb39d242b406f68b86d6ef6d1bb6494f8815f4ba06e4678dda0bc0`.
- Текущий headless: `current-825.exe`, SHA-256
  `3b2efc1749d767ab0ae7ce579a96ac3bddf05257978d4ea79852e4f39c928b53`.
- Текущий native: `current-825-blast.exe`, SHA-256
  `daea401eb8ddb8e0a843a2589aebe767fa02f49a16cbdf28a623c948575673d8`.

## Строгий headless A/B

World AI-vs-AI, две пары по1600 тиков в порядке после/до и до/после,
physics4/navigation8/Rayon4. Все36 нетайминговых полей каждого тика совпали
в обеих сериях.

| Показатель | Baseline | ADR-825 | Изменение | Относительная скорость |
|---|---:|---:|---:|---:|
|Средний тик|36,291 мс|33,557 мс|−7,53%|×1,081|
|Позднее окно1201–1600|53,042 мс|46,880 мс|−11,62%|×1,131|
|`debris`|17,255 мс|14,231 мс|−17,53%|×1,213|
|`motion`|5,339 мс|5,377 мс|+0,73%|×0,993|
|`rubble`|4,217 мс|4,312 мс|+2,26%|×0,978|

По отдельным сериям полный mean:36,251→32,852 и36,331→34,261 мс.
Вторая серия медленнее первой, поэтому прежняя оценка накопленных18–21% не
используется как строгий результат. Этот matched A/B заменяет её значениями
−7,53% среднего и−11,62% позднего тика.

## Текущий native

60-секундный World AI-vs-AI, Vulkan,3840×1949,focused=true,Windowed,
AutoNoVsync,compute4/physics4/navigation8/Rayon4,saved tuning. Обычный запуск без
CorePhases. `ValidForComparison=true`; `AcceptanceEligible=false`, потому что
реального темпа20Гц нет. Первые запуски с competing process/focus changes были
повторены и в эти результаты не входят.

| Метрика | Текущий native | Исторический427 | Разница |
|---|---:|---:|---:|
|Среднее кадра|33,550 мс|38,890 мс|−13,73%|
|Средний FPS|29,81|25,71|+15,95%|
|p50 / p95 / p99 кадра|27,217 / 73,596 / 104,386 мс|37,701 / 84,068 / 115,677 мс|p99−9,76%|
|Худший кадр|125,185 мс|142,222 мс|−11,98%|
|1% low|8,96 FPS|8,64 FPS|+3,65%|
|Кадров длиннее33,333 мс|45,65%|54,1%|−8,45 п.п.|
|Тиков за60с|1026|968|+5,99%|
|Долг на выходе|19|19|без изменения|
|Simulation realtime ratio|0,8514|≈0,8067|20Гц не достигнуто|

`simulation_core`: p50=13,686,p95=41,314,p99=69,417,max86,368мс.
`fragments`: p50=3,573,p95=18,154,p99=20,976,max23,299мс.

Новый native замер действителен для текущего binary, но сравнение427 историческое:
это не одновременный matched native A/B, а игровые данные продолжали меняться.
Поэтому +15,95% FPS — ориентир, а не строгий суммарный FPS-speedup. Текущие
29,81 FPS и все остальные текущие значения измерены непосредственно.

## Итог

Строго подтверждённое накопленное улучшение headless: средний тик−7,53%, поздний
тик−11,62%, debris−17,53%, все36 игровых полей прежние. Текущий native измерен:
29,81 FPS,p99 104,386мс,1% low8,96 FPS. Симуляция всё ещё отстаёт от20Гц;
главная retained-history работа не завершена.

## Воспроизведение

```bash
RAYON_NUM_THREADS=4 ./out/optimization-444/current-825.exe \
  --world --ai-vs-ai --ticks 1600 --physics-workers 4 --workers 8 \
  --tick-csv out/optimization-444/after-1600-1.csv
RAYON_NUM_THREADS=4 ./out/optimization-401/baseline/bench_battle-before.exe \
  --world --ai-vs-ai --ticks 1600 --physics-workers 4 --workers 8 \
  --tick-csv out/optimization-444/before-1600-1.csv

WGPU_BACKEND=vulkan powershell -NoProfile -ExecutionPolicy Bypass \
  -File scripts/profile-range.ps1 -Seconds 60 -Scenarios world-battle \
  -SkipBuild -BinaryPath out/optimization-444/current-825-blast.exe \
  -Output out/optimization-444/native-current-4 -SavedTuning \
  -NavigationWorkers 8 -PhysicsWorkers 4 -RayonWorkers 4 -ComputeWorkers 4 \
  -WindowWidth 3840 -WindowHeight 2160 -Maximized -Foreground
```
