# ADR-561 — подготовка входов параллельного contact index

04.10.2026. Родитель `24dc488`; исходные 13 WIP entries сохранены отдельно
от коммита. Изменён только transient scratch в `sim-core::burst`, физические
входы, баланс, частота симуляции, лимиты и существующие pools прежние.

## Устранённая работа

Восемь stripe jobs раньше каждый просматривали полный массив selected bounds.
Теперь один проход готовит reusable список для каждого X residue modulo eight.
Первые восемь X cells box определяют все его полосы, включая negative coordinates
и boxes шире восьми cells. Requests добавляются в исходном fragment order.
Прежние jobs заполняют свои tables/list slots; indexed zip не делит mutable
contact list между workers. Порядок индексов в каждой cell и wanted membership
полностью прежние, merge/reduce/sort не добавлены.

Это сокращение повторных scans, а не новая схема load balancing. Число stripes
и physics workers не меняется; более широкие boxes дают до восьми requests.
Scratch содержит только source index/bounds, до восьми записей на выбранный
fragment; Vec capacities переиспользуются. Lists очищаются перед каждым build,
contact tables — прежним clear. Scratch не сериализуется и не является физикой.

Новый observer interval `physics_cell_partition` отделяет последовательную
подготовку от прежних `physics_cells` leaves. Existing generic physics family
collector его принимает; формат логов, tooling и новые collectors не меняются.
Parent settled_cells уже включает partition и join: их нельзя складывать.

## CPU AI-vs-AI: два сравнения

Последовательно before → after → after → before, каждый 1800 ticks,
`RAYON_NUM_THREADS=8`, `--world --ai-vs-ai --workers 8`; без параллельных сборок
и тестов. Все alive/base/shot/damage outcomes и hash `bdfb97cef98d13e2` совпали.

| Run | Mean tick, µs | Tick p99, µs | Tick max, µs | Mean debris, µs | Mean cells ticks 1501…1800, µs |
|---|---:|---:|---:|---:|---:|
| before 1 | 10288.580 | 17631 | 36951 | 2353.885 | 247 |
| after 1 | 10343.838 | 18070 | 36949 | 2338.221 | 209 |
| after 2 | 10347.018 | 18040 | 36868 | 2344.012 | 207 |
| before 2 | 10371.101 | 17877 | 37375 | 2397.454 | 253 |

Cells parent с partition/join в позднем окне −15.38% / −18.18%.
Это значения шестого элемента штатного settled_phases_window, округлённые
bench до µs. Full tick +0.537% / −0.232%, p99 +2.49% / +0.91%:
общего CPU gain не доказано. Другие фазы/пики не атрибутируются этой правке.

Первый sanity CPU pair без `--ai-vs-ai` имеет другой hash `063d02cdc4dec589`;
он хранится отдельно, не смешивается с основным боем или native captures.

## Ordinary native: два сравнения

Четыре последовательных capture по 120 с в том же порядке, 60 юнитов,
1600×900, AutoNoVsync, physics=8, navigation=8, compute=4, обычные эффекты,
без core/render/task collectors и concurrent builds/tests/analysis.

| Run | FPS по begin intervals | Mean frame, ms | p99, ms | Max, ms | Core/actual step, ms |
|---|---:|---:|---:|---:|---:|
| before 1 | 84.961 | 11.7701 | 30.2348 | 49.0892 | 14.09453 |
| after 1 | 85.049 | 11.7579 | 30.6015 | 60.9791 | 14.17628 |
| after 2 | 85.234 | 11.7324 | 30.4713 | 51.4889 | 14.15852 |
| before 2 | 85.300 | 11.7233 | 30.4544 | 50.9182 | 14.18962 |

Оба сравнения eligible без удаления metadata keys. Во всех четырёх captures
3600 actual steps, tempo 1×, hash `bcc1f58c226d1721` и полные PROFILE fuel,
stream/damage/melting, battle shots/damage/alive lines совпали. FPS +0.103%
/ −0.077%: общего ускорения нет. Core/step +0.580% / −0.219%; p99 и max
хуже в обеих парах. Нельзя объявлять FPS или hitch fix по локальному CPU gain.

### Финальная сборка и редкий пик

После type alias cleanup финальная сборка пересобрана и проверена ещё 120 с,
затем контрольный before ещё 120 с; это дополнительные eligible runs, не
замена первых четырёх и не основание выбрать лучшие результаты.

| Run | FPS | Mean frame, ms | p99, ms | Max, ms | Core/step, ms |
|---|---:|---:|---:|---:|---:|
| final after | 82.038 | 12.1894 | 32.2108 | 248.2062 | 14.65643 |
| late before | 77.753 | 12.8612 | 32.4472 | 75.4965 | 14.78203 |

Полные gameplay lines/hash и 3600 steps совпали во всех шести ordinary runs.
Условия comparison gate сохранены, но поздняя серия отличается от ранней:
причина межзапускового изменения FPS не установлена. Нельзя приписывать
82→85 или 77→82 эффекту алгоритма. Самый большой новый кадр следует за
tick 3001: previous core 236.6501 ms, measured slice 243.522 ms, outside
slices 4.684 ms. Причина дорогого core в этом ordinary run неизвестна.

Отдельный final diagnostic 120 с, `-CorePhases`: 3600 traced/actual steps,
1×, coverage без missing ticks; evidence valid, FPS acceptance false.
7149 partition calls на main OS ID 581448 и 57192 stripe jobs: 1096 serial
jobs на main, остальные на восьми OS workers (393284, 587472, 445432,
349364, 427376, 409080, 675536, 537356). Partition inclusive wall 369.9082 ms
(средний вызов 51.743 µs, max 316 µs); cells inclusive 4123.5448 ms across
threads, max job 788.6 µs. Эти суммы не wall parent и не balanced utilization.

Tick 3001 в diagnostic занимает 18.2441 ms: ordinary 236.65 ms не повторился,
поэтому источник не атрибутирован. Другие пики сохранены: tick 2518 total
68.4734 ms, rubble 37.0597 ms; tick 2245 total 53.4841 ms, debris 40.8177 ms.
Task-family longest samples недостаточны, чтобы объяснить весь parent:
нужны точные serial/dispatch/join boundaries и OS scheduling/wait stacks.
Нулевые/квантованные CPU counters не доказывают lock wait.
WBPERF-001/027 остаются открытыми; новый partition marker виден и отделён.

## Проверки и артефакты

Полный release `sim-core --lib`: 562 passed, четыре прежних ignored.
Существующий exact differential сравнивает ordered contact map/list с серийным
эталоном при 1/2/4/8 workers, negative cells, wide boxes, empty/repeated masks
и reused lists. После Clippy cleanup (только type alias для request tuple)
этот differential повторён. Новые type-complexity замечания устранены;
unrelated прежние warnings не исправляются в этом коммите.

Алгоритм A/B замерен до type alias cleanup. Frozen CPU before
`out/fix559/final-bench.exe`, SHA256
`35654E2D5E151B02235A4B29F57505029AAC9F2FF5E5D747B8992E3AB81A4939`;
CPU after `out/fix561/partition-bench.exe`, SHA256
`2EDB09686FBAED77C023049562B83F72020BE4994E0877100EE98E7518C0F501`.
Native before `out/fix560/dense-history.exe`, SHA256
`BF576F24DA0EE6CC20360BA73E9AC71C0A82F0D041DF919E4157A317A3A75D7C`;
after `out/fix561/partition.exe`, SHA256
`3BEF8D2E9469D4DA379BE8E92B5954B874FB1E24536644D71EA735EEE83EF177`.
Build manifests attest перечисленные refreshed hashes, не clean dependency
closure; прежние WIP входят в обе стороны. Raw files сохранены в `out/fix561/`:
`{before-ai,partition-ai,partition-ai-repeat,before-ai-repeat}-{ticks.csv,bench.log}`,
`{before-native-120,after-native-120,after-native-repeat-120,before-native-repeat-120}`,
`cpu-ai-summary.json`, `final-native-summary.json`, `comparisons.json`.
Финальный runtime `out/fix561/partition-final.exe` и `target/release/blast.exe`,
SHA256 `93160817B7249A7863423699ACFF93C436336E7212E101D4C05ED008B2980879`.
Дополнительные raw captures: `final-native-120`, `final-control-native-120`,
`diagnostic-120`; у каждого CSV/log/manifest/analysis и OS process observations.
Финальный refreshed burst hash
`B120E84DFCDB987999BE0CE12C22304FDC498538BE5CF7886F0FD685F7C16DC2`.

## Что остаётся пределом

WBPERF-004/014 получают только локальное сокращение contact work. WBPERF-001
получает отдельный partition span. Balanced CPU, rare stalls WBPERF-027 и
165 FPS не закрыты. Main frame синхронно ждёт ~14 ms authoritative tick:
WBPERF-002 остаётся главным архитектурным пунктом. Worker step с same-frame
join не решает ожидание. Нужны owned sim executor, ordered commands с
tick/generation/sequence и bounded completed read model без FullWorld clone,
пропуска terrain/hit/death событий или снижения 30 Hz.
