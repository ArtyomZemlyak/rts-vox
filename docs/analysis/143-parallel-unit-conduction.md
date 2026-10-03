# ADR-549 — параллельные чтения теплопроводности юнитов

03.10.2026, HEAD до изменения `13695f0`, продолжение
[142](142-debris-support-lower-bound.md), WBPERF-012/001.

## Принятый вариант

Пакет от 256 источников читает geometry и старое тепло соседей в
существующем physics pool. Indexed collect сохраняет cursor order.
Минимальная leaf batch — 128 источников: штатный полный пакет 512
образует четыре задания; более крупные configured пакеты могут иметь
больше leaves. Малые/однопоточные пакеты серийны. Число потоков пула
прежнее, восемь в измеренном world-battle.

Capacity/admission, удаление stale sources, outflow, cooling, rounding,
delta commit и melting сохраняют исходный порядок. Debit источника
накапливается локально и один раз записывается в delta. Геометрия и
тепло соседей неизменны до плавления; пустой источник освобождает слот
именно на своей прежней позиции серийного прохода. Его старое тепло
не требуется подготовленному соседу, потому что geometry check такого
соседа уже отклоняет пустой voxel.

Новых пулов, persistent caches, зависимостей, snapshot fields, изменений
лимитов работы, частоты тика, баланса или визуального качества нет.

## CPU и обычный native бой

World-battle AI против AI, 30×30, 3600 тиков, physics/navigation=8,
штатное оружие/разрушения/эффекты. Frozen runs последовательные, без
конкурирующих сборок, тестов или тяжёлого анализа. Данные — `out/fix549`,
`bench-summary.json`, `native-summary.json`, `summarize.py`,
`inspect-evidence.py`, исходные logs/CSV/manifests.

CPU-пара `before-four` / `after-four`:

| Метрика | До | После |
|---|---:|---:|
| Conduction, среднее окон, мкс | 258,833 | 198,833 |
| Полный unit heat, мкс | 333,333 | 279,917 |
| Heat flush, среднее, мкс | 820 | 769 |
| Полный тик, среднее, мс | 13,119355 | 12,633736 |
| Полный тик, p99, мс | 21,682 | 20,656 |
| Максимальный тик, мс | 49,510 | 48,254 |

Conduction быстрее на 23,2%, unit heat на 16,0%, heat flush на 6,2%.
Полный тик быстрее на 3,7% в одной паре; часть разницы приходится на
другие фазы, особенно debris 3,009→2,791 мс. Нельзя всю разницу приписывать
тепловым чтениям. Counters tick/searching/moving/blocked/fragments/flying/
chunks/felling совпадают каждый тик, final fired/damage/alive/bases/state
одинаковы, hash `bcc1f58c226d1721`.

Две ordinary native пары по 120 с. Первая old→new, вторая new→old.
Одинаковые 1600×900/AutoNoVsync, compute=4, physics/nav=8. Core/GPU/render
diagnostic collectors выключены. Все native/offline/strict pair gates
проходят; каждый run исполнил 3600 тиков, tempo ~0,9995…0,9996×, тот же hash.

| Метрика | До / 1 | После / 1 | До / 2 | После / 2 |
|---|---:|---:|---:|---:|
| FPS = 1000 / mean frame | 78,020 | 78,595 | 77,988 | 79,370 |
| Mean frame, мс | 12,817262 | 12,723527 | 12,822530 | 12,599162 |
| p99 frame, мс | 33,3092 | 32,7678 | 32,6959 | 32,4123 |
| Max frame, мс | 82,7222 | 64,4901 | 76,7261 | 64,6096 |
| Core/step, мс | 14,171392 | 13,960698 | 14,141351 | 13,914638 |

Observed FPS выигрыш 0,74–1,77%, core/step −1,49…1,60%. Это небольшой
локальный эффект, не доказательство универсального выигрыша и не исправление
редких stalls. 165 FPS/6,06 мс остаются недостигнутыми.

## Отклонённый вариант: восемь заданий

Leaf minimum 64 давал conduction 259…261→178…180 мкс в двух CPU-парах,
полный tick −1,6…1,9%, одинаковые counters/hash. Но две ordinary native
пары (вторая в обратном порядке) дали FPS 78,42→76,37 и 78,43→77,19:
регрессия 2,6% и 1,6%. Первый run имел max frame 211,901 мс; row6355,
tick1736, предыдущий core175,249 и measured slice195,781 мс. Второй такой
пик не повторил. Ordinary запись не позволяет назвать конкретный worker
или причину ожидания. Версия отклонена несмотря на более быстрый CPU hot phase.

Более крупные leaves проверены именно из-за этой интеграционной регрессии.
Накладные расходы/конкуренция — гипотеза, не установленная OS wait-причина.
Измеренная поправка minimum 128 принята по полному native бою.

Контроль `serial-reads-bench.exe` отличается от варианта 64 только `true`
на `false` у вызова conduction_delta; остальной physics pool остаётся 8.
3600 тиков, те же counters/state/hash: conduction 246,5 мкс, unit heat 322,75
мкс, тик 12,796791 мс, p99 21,432, max 71,557 мс. Локальный debit/подготовка
без потоков даёт небольшой выигрыш; потоковые чтения128 примерно на 19%
быстрее этого отдельного контроля. Межзапусковые вариации других фаз не
приписываются самому переключателю.

Raw eight-leaf данные: `before`/`after`, `before-repeat`/`after-repeat`,
`after-native-120`, `after-native-repeat-120`, paired before directories.
Дополнительная `aux-*` CPU-пара сохранена, но исключена из принятой
таблицы: короткие Python tests 0,734 с пересеклись с её before run.

## Наблюдаемость

Новые physics scope families, stage=5: `physics_unit_heat` (весь проход),
`physics_unit_heat_read` (реальные Rayon leaves либо serial packet),
`physics_unit_heat_admit` (последовательные delta/admission). No-op
observer не содержит clocks/IO/OS queries; таймеры, CPU/cycles и thread ID
принадлежат прежнему render profiler. Bounded trace хранит полные counters
и longest scope на thread/family/stage; старые schema 3 данные совместимы.
Parent/child wall inclusive: их нельзя складывать как elapsed time фазы
или CPU utilization. Final thermal field commit/melting входят в parent,
а admission span измеряет создание delta и лимиты.

Принятый вариант `four-core-30`: 900 тиков/900 parent и admission jobs,
2638 read jobs; 11 observed OS threads чтений включают caller threads,
это не 11 physics workers. Read-only threads вне четырёх parent callers —
семь; actual pool size 8. Read cycles 514984770, max read scope 375800 нс,
max parent 1017200 нс. Полнота/native/offline проходят. Diagnostic FPS не
используется в A/B. В rejected eight-leaf trace 60 с: 1800 parent/admission
jobs, 12275 read jobs; read scopes видны на всех восьми физических workers
плюс caller threads. Сам процент общей загрузки CPU не доказывает idle pool.
Fire receiver jobs ADR-547 всё ещё без отдельных child scopes.

## Проверки и provenance

Полный release sim-core run — 560 passed, 4 ignored. Differential oracle:
768 serial/parallel cases, 1/4/8 workers, cursor wrap, capacity saturation,
cooling/conduction extremes, отверстия и gone sections. После изменения
leaf minimum 64→128 этот тест повторён (passed); полный бой/counters/hash
проверен CPU и native. Первый полный run упал только на новой fixture,
предполагавшей два тела вместо одного; второе тело теперь создаётся явно.
Остальные 559 тестов уже проходили, полный повтор прошёл целиком.
95 analyzer regression tests проходят, включая stage 5 readers/admission.
Final release clippy без новых warnings (три прежних voxel/nav/sim-core).
Seed 42, ticks 1000, units 5000, threads 1 CLI hash `66041a9f60189841` прежний.
Старые пять architecture failures ADR-547 не исправлены.

Во всех сравниваемых builds присутствуют одинаковые 13 прежних WIP entries
ADR-523 selection/laser/walker feet; они не входят в этот коммит. HEAD и
SourceAtLaunch не доказывают исходники exe. После capture eight-leaf
`after.exe` сохранён как `eight-leaf.exe`; идентичность задаёт stored SHA,
а финальный `after.exe` — копия принятого `four-leaf.exe`.

| Frozen exe | SHA256 |
|---|---|
| before.exe | `4021acbe2ae9d488287049c3b3f0daa33c474d00915b8539fc7065daa3168c0b` |
| four-leaf.exe / after.exe | `4e3124454f6e1d8ba0730f7208f412853543d142c17259e54786b92d99d025aa` |
| before-bench.exe | `5bff243c980dadecfe1368ca3126f39877b6c952e99f7fb3f1029fd647d745ac` |
| four-leaf-bench.exe | `f7a3eb97d8517bfde501e89034ec1ff0607a7f50289e522263fab04fb240be0b` |
| eight-leaf.exe | `dd743b4478e564a76086cacbd5bde29d2c1d1622ec9f27062949fed504677e08` |
| after-bench.exe (eight-leaf first pair) | `55464f8059d96b20ab3ceeed573867f5feb99e5fb9fe824ba025c0c42355f22e` |
| eight-leaf-final-bench.exe (second pair) | `f786fb0bc4ebb02f627a69b59a3c4611f04a71cbc34896764f4ebe9f3aa751da` |
| serial-reads-bench.exe | `cdbe1a23202bf261e53bcce3f886a965f5585ea488fd22382d0bfe4929609fc3` |

Главный следующий этап — WBPERF-002: убрать последовательную симуляцию
из критического пути кадра. Движение (~5 мс), debris и редкие stalls тоже
остаются открыты; одним увеличением количества параллельных jobs это не
решается. Поведение, энергия и визуальное качество этим шагом сохранены.
