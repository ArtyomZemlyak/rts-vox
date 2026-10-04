# ADR-564 — повторное использование параллельно рассчитанных границ обломков

04.10.2026, исходный `main` `ba355af`, WBPERF-004/012.
Границы обломков из неизменного снимка фазы используются повторно при
пробуждении, проверке опоры и начале движения. Это уменьшает работу
существующих physics workers. Общего ускорения нативного кадра замеры
не подтвердили; 165 FPS, равномерная загрузка CPU и редкие зависания открыты.

## Механизм и граница применимости

`Settled::around_bounds_into` уже рассчитывает world boxes всех fragments
пакетами по256 в постоянном physics pool. После этого wake снова вычислял
локальные границы, включая shape, mm и18 Q32 endpoint multiplications.
Translation и support повторяли этот расчёт.

`Settled::fragment_bounds` вычитает прежний центр из готовых world boxes:
`(position.add(bound)).sub(position)` возвращает те же биты, поскольку
обе операции Fx используют wrapping arithmetic. Это не формула
centre/extent с другим округлением. Пустой/непостроенный снимок и corpse
queries сохраняют прямой `Fragment::bounds`.

Wake меняет только velocity/resting. Translation читает собственную позу
начала фазы до eviction; другие workers изменяют только свои fragments.
Substeps получают уже рассчитанные local bounds: eviction меняет position,
а shape/basis остаются прежними до tumble после substeps. Кеш нельзя
использовать после изменения позы или формы; следующий tick строит boxes
заново. Проверка support в rubble также читает неизменный вход своего запроса.

Новых полей scratch, allocations, pools, threads, locks, clocks, gameplay
ceilings или snapshot/save data нет. Indexed scheduling, порядок выбора
контактов, flying claims и ordered merge остаются прежними. Это reuse
результата параллельной подготовки, не добавление parallel jobs ради их числа.

## Отвергнутый вариант: wake grid

Сначала проверен пространственный индекс hulls с16-unit cells, membership
bit masks до64 hulls и точными прежними проверками в порядке индексов.
Два targeted tests покрыли повороты, voxel holes, отрицательные координаты,
границы ячеек, bit63 и fallback для больших/крайних запросов. Они прошли,
но CPU full battle показал wake891,33→956,17 µs, debris4220,25→4372,43 µs,
total15899,16→16277,01 µs. Вариант отвергнут; его код и tests не вошли.
Raw — `rejected-wake-grid-burst.rs`, `grid-core.*`, `before-core.*`.

Поздний baseline оказался быстрее раннего на~20%; это дрейф среды,
а не выигрыш последующего reuse. Сравнения ниже используют соседние свежие
controls. Этот эпизод подтверждает необходимость ADR-563 OS pressure;
старые CPU-only данные не показывают всю внешнюю нагрузку.

## Корректность

50 debug physics tests проходят. Независимый eight-corner geometry oracle
расширен проверкой обратного преобразования из world box при дробных
позициях и Fx MIN/MAX, разных mm, shapes и rotations. Проверены unbuilt
cache и отсутствующий fragment index. Пять generation rebuilds проверяют
точность local bounds после pose changes, empty queries и смены input.

Новый differential test сравнивает90 translation steps с cached/direct
bounds после eviction, bounce, tumble и landing. Полный flying replay
расширен до1/2/4/8 workers; прежние ordered wake/merge/contact tests проходят.
Release golden4 и save/replay19 проходят, save format/hash не менялись.
Первый debug golden запуск прерван на длительном worker replay после трёх
завершённых cases; это не полный passed run. Release rerun завершён полностью.

Fmt проходит. Clippy: только прежние warnings в voxel/world.rs,
nav/vehicle.rs и исходном WIP blast.rs; изменённый код без новых warnings.
Исходные13 WIP сохранены по bytes, mixed docs получают отдельный owned append.

## CPU: две соседние пары по3600 ticks

Stock full-map 30×30, AI vs AI, physics/navigation8/8. Порядок A:
reuse→before; B: before→reuse. Во время timed runs не было наших
builds/tests/edits/тяжёлого анализа. Это не исключает чужую нагрузку.

| Среднее µs | Before A | Reuse A | Before B | Reuse B |
|---|---:|---:|---:|---:|
| Wake, включая подготовку | 579,33 | 488,08 | 620,58 | 491,00 |
| Wake, последние1200 ticks | 859,75 | 708,25 | 874,50 | 707,75 |
| Debris | 2893,52 | 2732,65 | 3029,45 | 2755,74 |
| Полный tick | 12639,67 | 12486,88 | 12931,66 | 12519,05 |
| Motion, неизменённая | 4951,10 | 4981,95 | 5049,47 | 4990,24 |
| Rubble, неизменённая | 1011,43 | 1009,94 | 1040,14 | 1010,41 |
| Tick p99 | 19789 | 19771 | 21840 | 19771 |

Wake mean −15,75% / −20,88%, late wake −17,62% / −19,07%; debris
mean −5,56% / −9,04%. Общий tick −1,21% / −3,19%; весь второй общий
выигрыш нельзя приписать reuse, соседние неизменённые фазы тоже меняются.
В Reuse A rubble max19,948мс против2,871мс control; пик не скрыт,
устойчивого устранения latency tails нет.

Все шесть CPU runs, включая grid screening: counters каждого из3600
тиков и full gameplay/unit lines совпали; hash `bcc1f58c226d1721`.
Baseline CPU frozen: ADR-563 `bounds-before-bench.exe`, SHA256
`EDF4FFADE0BF87055D54E328A1000726BE732E0A1915FDC6EC9348720AA941BE`;
его burst source hash совпадает с initial этого шага. CPU after:
`A5390747ED17E326A8E471D804F7351EF16D911C4DC913FCEAD4F2506D475564`.

## Native: локальный CPU gain не равен FPS gain

1600×900, AutoNoVsync, compute4, physics/nav8/8,120с, TargetFps165;
обычные runs без CorePhases/OsPressure/RenderTimeline. Обе пары eligible,
3600 ticks/30Hz, exact fuel/stream/battle/hash, тот же полный workload.
Порядок A reuse→before, B before→reuse.

| Ordinary | FPS | p99 frame, ms | Max frame, ms | Core/step, ms |
|---|---:|---:|---:|---:|
| Before A | 87,395 | 30,072 | 51,270 | 13,867 |
| Reuse A | 65,855 | 46,303 | 73,014 | 16,950 |
| Before B | 87,092 | 29,798 | 48,914 | 13,844 |
| Reuse B | 86,644 | 29,952 | 85,959 | 13,864 |

Первый after сильно медленнее (FPS −24,65%, core +22,23%), второй
возвращается примерно к baseline (FPS −0,514%, core +0,145%). Большая
просадка не повторилась, но причина первого результата не установлена:
ordinary captures не имеют system pressure/worker scopes/OS stacks.
Его33 stall episodes сохранены и не объявлены ни исправленными, ни
доказанным эффектом среды. Во второй паре max frame также хуже.
FPS gain и улучшение худших кадров не доказаны. Изменение сохраняется
как локальная оптимизация геометрии, не как закрытие полного FPS пункта.

Baseline native — ADR-562 `camera-final.exe`, SHA256
`8F013BBFE8990D15369747F9428CB9AE2BC6BFC3609EC4639C8D42A553F6BC8F`.
After/installed `target/release/blast.exe`:
`E59DEA7E2243C940BE6AFD61951657B06D8A925884538AF85F686BD30A692AC9`.
After listed burst source SHA256
`1D5EF70C1EBE08CA8FC799A462FCCB2F204AA5CE53F14662E735DABFA137E1E8`.
Build manifests фиксируют перечисленные refreshed sources, не полную
dependency closure и не clean checkout: обе стороны включают одинаковые WIP.

## Камера и pressure diagnostic

`reuse-pressure-tour-120`: OsPressure+CpuThreads+CameraTour+trigger40мс.
3600 ticks, strict tempo gate true, FPS acceptance false.21 segments,
151 published origins,0 blocked frames, max frame52,624мс,
longest sampled no-tick interval33,011мс. Это bases/pan/zoom autotour;
mouse/orbit и pixel QA не проверены. Gameplay/hash совпадают с ordinary runs.

115 целых in-window OS intervals, system busy mean45,23%/max56,44%,
sensor read mean122µs/max603,5µs, sampler work mean29,26мс/max66,59мс.
Warmup414,3µs, errors/timeout отсутствуют, counter discontinuities0.
Worker scopes отсутствуют; coarse thread CPU и phase ring не заменяют
scheduler traces. Отсутствие большого stall в одном туре не означает
исправления прежних двухсекундных freezes. Тур не объясняет первый
ordinary after и не сравнивается с ним как FPS A/B.

Raw — `out/fix564/`: all CPU/native variants/build manifests,
core/native summaries, rejected grid, physics/clippy/fmt logs и
`.analysis.json/.md`. Первый неполный debug golden run явно отделён от
успешного release rerun. Ничего из raw captures не публикуется как код.

## Следующий приоритет

WBPERF-027: matching stationary pressure/phase/worker evidence для
повторяющейся просадки и OS scheduler stacks; не объяснять её одной
внутренней функцией по ordinary wall time. WBPERF-002: independent sim
executor и bounded completed read model без same-frame join. Основной
поток продолжает синхронно ждать tick, который длиннее165FPS frame budget.
More workers/reuse не устраняют эту архитектурную границу.
WBPERF-004/012 выполнены только локально; CPU balance, tails и165FPS открыты.
