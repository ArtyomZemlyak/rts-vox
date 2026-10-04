# ADR-555 — параллельная подготовка навигационных обломков

04.10.2026, исходный HEAD `58ff490`, WBPERF-004/014/001/027.
Артефакты: `out/fix555`, frozen binaries/build manifests, tick CSV,
`core-summary.json`, `native-summary.json`, `worker-evidence.json`,
`control-provenance.json`, ordinary native и отдельный diagnostic dataset.
Прогоны последовательные, без конкурирующих сборок, тестов, правок
или тяжёлого анализа во время измерений.

## Изменение

`Bursts::rubble_observed` проверял опору каждого фрагмента и собирал
колонки последовательно. Теперь от 1024 fragments классификация
исполняется пакетами по 256 в существующем Rayon physics pool. Каждый
пакет владеет своими bounds и двумя списками: world-supported и stacked.
Indexed collect соединяет их в прежнем порядке. От 1024 подтверждённых
floor candidates отдельные пакеты по 256 собирают колонки и переиспользуют
собственный hull filter; caller соединяет результаты, сортирует и выбирает
прежний максимальный top каждой колонки. Меньшие входы последовательные.

Мир, hulls и fragments неизменны на протяжении запроса. Проверка stacked
support остаётся прежней, как и материал/yielding/chunk фильтры, rounding,
roof/flank exclusion, bounds и same-call видимость изменившихся опор.
Нет новых pools/threads, сохранённого кеша между тиками, work ceilings,
изменений баланса, состава боя, частоты симуляции или визуальных эффектов.
Это ускорение существующей фазы, не завершение цели 165 FPS.

## Корректность и наблюдаемость

Существующий ADR-479 oracle расширен до 1537 mixed fragments, поворотов,
движущихся hulls, крыш с voxel solid, yielding materials и уничтожения
опор между запросами. Весь Rubble совпадает на 1/2/4/8 workers.
Новая проверка отдельно покрывает 0/1023/1024/1537 floor pieces: обе
границы включения, пустой ввод, несколько пакетов и неполный последний.
Test-only Trace проверяет paired события на фактических calling threads.

`physics_rubble_floor` теперь отмечает реальные classify batches.
Новая семья `physics_rubble_gather` отмечает gather batches, включая
малый serial путь. `physics_rubble_columns` по-прежнему parent на caller:
reach/filter setup, gather/join, sort/dedup и строительство карты.
**Parent columns и дочерние gather scopes не складываются.**
Существующий profiler сохраняет totals всех jobs и longest scope каждой
OS thread/family/stage; у no-op observer нет собственных clocks/locks.

## Проверяемые сборки

Применён ADR-554 build-range-variant protocol. Before CPU build refreshed
burst, blast, range_motion и nav/vehicle; after refreshed burst.
Содержимое перечисленных inputs стабильно через build, executable frozen.
Это ограниченная аттестация перечисленных файлов, не всей dependency
closure/clean checkout. Прежний ADR-523 WIP одинаков и сохранён.

- CPU before: `95fc6c9a4e2ef18e9f5f961f2423e8c4d15b242c4b4bb1d9ac0110eff4c71ba5`.
- CPU after: `9db3141f90c3fdff2632b1cae64fbc265de1a4f7a0b39a10dc07e3430a8bec69`.
- Native control — reused frozen verified ADR-554 `out/fix554/tiles-verified.exe`:
  `39018510322501891f2e06a68c05cbf6fc5bd736560b22349a25cdef1c4b8821`.
  Его три listed source hashes совпали с нынешним baseline, исходный burst
  совпал с HEAD; это не новая полная build-source аттестация.
- Native after и `target/release/blast.exe`:
  `aa8b674b8417a13f89ceaddf805a4ab8d351b9eb93ed9b4f094cea5cd6516193`.

## CPU полный бой — повторяемое локальное ускорение

60 actors, AI обеих сторон, physics/nav=8, 3600 ticks, только bench phase
clocks. Порядок before→after→after→before. Every-tick counters и полные
final state/per-unit log evidence совпали; hash `bcc1f58c226d1721`.

| Среднее мс | До / 1 | После / 1 | До / 2 | После / 2 |
|---|---:|---:|---:|---:|
| Rubble | 1,249579 | 1,024381 | 1,261371 | 1,016558 |
| Полный tick | 13,186849 | 12,988117 | 13,366491 | 12,876193 |
| Motion | 5,098969 | 5,107572 | 5,151754 | 5,086731 |
| Debris | 3,100280 | 3,105249 | 3,173412 | 3,029320 |
| Rubble p99 | 2,432 | 1,960 | 2,463 | 1,944 |
| Rubble max | 2,761 | 2,293 | 3,267 | 3,994 |
| Tick p99 | 21,453 | 21,541 | 21,937 | 20,974 |
| Tick max | 50,716 | 51,237 | 54,136 | 50,387 |

Rubble mean −18,02% / −19,41%, p99 −19,4% / −21,1%.
Полный tick −1,51% / −3,67%; вторую общую разницу нельзя целиком
приписать rubble: неизменённые motion/debris тоже изменили длительность.
After-repeat rubble max 3,994 мс хуже control 3,267 мс; пиковый результат
не скрыт и не объявлен доказательством устранения зависаний.

## Ordinary native — общий FPS выигрыш не подтверждён

Четыре runs по 120 с: before→after→after→before. 1600×900 AutoNoVsync,
compute=4, physics/nav=8, штатные эффекты и process sampler, подробные
коллекторы выключены. Все manifest/offline pair gates проходят; каждый
run делает 3600 steps в реальном темпе, full gameplay evidence одинаковы,
тот же hash. Eligibility условий не означает достижения 165 FPS.

| Метрика | До / 1 | После / 1 | До / 2 | После / 2 |
|---|---:|---:|---:|---:|
| FPS | 79,4771 | 81,9392 | 86,5907 | 84,8056 |
| Mean frame, мс | 12,582234 | 12,204168 | 11,548579 | 11,791669 |
| p99 frame, мс | 32,0747 | 31,3116 | 30,6285 | 30,0874 |
| Max frame, мс | 65,7675 | 65,1431 | 63,5277 | 63,2657 |
| Core/step, мс | 14,779899 | 14,530802 | 14,123352 | 14,050810 |

FPS +3,10% / **−2,06%**. Core/step −1,69% / −0,51%; frame p99
лучше в обеих парах, но устойчивого роста FPS нет. Whole-frame latency
не приписывать автоматически новой геометрии; причина обратной FPS
просадки здесь не установлена. Изменение принято как повторяемое
ускорение CPU rubble и перенос независимой работы на physics workers;
не как whole-battle FPS/stalls acceptance. Снижение core малое, без
обычных per-phase native scopes его причинная атрибуция ограничена.

## Native execution evidence — восемь workers, балансировка открыта

Отдельный 60-секундный after diagnostic с CorePhases/CpuThreads:
1800 traced/measured steps, physics coverage missing=0, unmapped/missing
clock/identity mismatch=0; realtime 0,99959×. Valid evidence, **не FPS
приёмка** из-за подробного observer. Hash на 1800 ticks `bdfb97cef98d13e2`
не сравнивается с final hash 3600-тиковых runs.

Columns parent выполнялся на четырёх tick-caller OS IDs (Bevy compute
workers). Восемь других OS IDs исполняли classify/gather jobs:
`118560, 133932, 17020, 196756, 227192, 241988, 647196, 681652`.
Classify: 88704 jobs на physics workers, 95 малых calls на tick callers;
gather: 4844 jobs на physics workers, 912 малых calls на tick callers.
На каждом physics worker есть положительные calling-thread cycle deltas
обеих семей. Это подтверждает реальное выполнение, а не лишь объявленный
размер pool. Worker cycles: classify 2300676730, gather 572162300.

Работа **не равномерная**: max/min aggregate worker cycles за capture
13,19× у classify и 5,20× у gather. Это распределение за 1800 ticks,
не доказательство конкретного fork/join tail или причины frame stall.
QueryThreadTimes имеет грубую квантовку 15,625 мс; суммы малых job CPU
могут превышать wall, поэтому не вычислять utilization из этих deltas.
Cycle counters/OS lifetimes и intervals сохраняются отдельно.
Средний diagnostic rubble 0,759710 мс включает observer и только первые
1800 ticks; не сравнивать с ordinary/full 3600 CPU временем как speedup.

## Проверки и следующий приоритет

- sim-core release lib: **561 passed, 4 прежних ignored**, 114,95 с;
- Clippy release lib проходит с тремя прежними dependency/blast warnings,
  собственное изменение предупреждений не добавляет;
- own Rustfmt и diff checks проходят;
- Python analyzer: **95 passed**; PowerShell tools: **54 cases** и parser checks;
- четыре ordinary datasets и diagnostic читаются analyzer и проходят
  соответствующие evidence gates; full ordinary gameplay и CPU every-tick
  counters сохраняются.

WBPERF-004/014 частично: floor/gather parallel, stacked filtering,
sort/dedup/build и часть query preparation остаются последовательными.
Следующий кандидат — измерить per-join critical worker tail и распределение
тяжёлых fragments по пакетам; изменение гранулярности требует нового A/B.
WBPERF-002 (owned sim вне frame через compact read model, не full clone),
001/027 (native critical path и stalls), balanced CPU и 165 FPS открыты.
