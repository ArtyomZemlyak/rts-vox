# Параллельная физика: отвергнутые варианты и диагностика внешней нагрузки

04.10.2026, исходный `main` `fc32335`, ADR-563. WBPERF-001/004/012/027.
Пять вариантов физики проверены и отклонены. Production алгоритм,
пулы, число workers, игровой баланс и fixed-point расчёты сохранены.
Принято улучшение диагностики: `-OsPressure` показывает общую нагрузку
CPU, давление памяти и CPU читаемых процессов рядом с эпизодами зависаний.
165 FPS и устранение длинных задержек этим шагом не достигнуты.

## Условия и воспроизводимость

Full-map world battle 30 на 30, AI против AI, stock tuning, 3600 тиков.
CPU: physics=8, navigation=8. Native: 1600×900, AutoNoVsync, compute=4,
120 секунд, TargetFps=165. Во время timed runs не запускались наши
builds/tests/analysis/video. Это не гарантирует отсутствия чужой нагрузки.
Исходные 13 WIP сохранены; стороны A/B включают одинаковый WIP код.
Измеряется текущее рабочее дерево, не clean checkout `main`.

Frozen executables имеют SHA256 и build manifests. Восстановленный
`burst.rs` получает свежий timestamp перед сборкой. Raw CSV/logs,
отвергнутые исходники и manifests находятся в `out/fix563/`.
`summarize.py` сверяет counters каждого из 3600 тиков, итоговый hash,
damage, shots и полные unit lines; native — fuel/stream/battle/hash и gate.

Baseline native — `out/fix562/camera-final.exe`, SHA256
`8F013BBFE8990D15369747F9428CB9AE2BC6BFC3609EC4639C8D42A553F6BC8F`.
Он восстановлен в `target/release/blast.exe`. Отвергнутый native reuse —
`reuse-native.exe`, SHA256
`E45833525C8A78EA6E52668649B66BE64F88D2B3C4E536E684399C945FA247F2`.

## CPU screening: все варианты отклонены

| CPU, mean µs | Полный тик | Обломки | Вариант |
|---|---:|---:|---|
| Initial before | 12818,78 | 3064,79 | Исходный алгоритм |
| Identity bounds | 12916,12 | 3086,48 | Пропуск ONE/zero умножений |
| Identity repeat | 12842,23 | 3049,49 | Обратный порядок пары |
| Initial before repeat | 12897,67 | 3088,05 | Нет устойчивого identity gain |
| Word intersections | 12843,34 | 3040,27 | Candidate selection 432,25 → 420,33 µs |
| Word intersections + contact fill | 12884,54 | 3112,89 | Contact fill 232,92 → 254,75 µs |
| Persistent outputs + fixed chunks256 | 13199,25 | 3276,92 | Translation 591,67 → 727,67 µs |
| Persistent flat outputs | 13045,28 | 3101,96 | Прежний indexed scheduling |
| Fresh before, позднее | 13265,83 | 3234,01 | Дрейф относительно ранних запусков |
| Persistent flat outputs repeat | 13258,81 | 3199,37 | Тик почти равен свежему before |

Все CPU варианты дали `bcc1f58c226d1721` и одинаковые counters всех
3600 тиков, unit states, shots и damage. Identity проверялся на пустых
формах, крайних Fx, обратных bounds и одно-битном отклонении basis.
Word path проверялся на отрицательных координатах/64-bit boundaries;
контактные списки совпали при 1/2/4/8 workers.

Fixed chunks256 увеличили стоимость translation примерно на 23%.
Большее число задач само по себе не означает полезную параллелизацию.
Плотные обычные куски не выигрывают от сложного bit enumeration.
Identity и word selection не дали устойчивого общего выигрыша.

Поздний исходный алгоритм тоже стал медленнее раннего baseline.
В соседнем late comparison reuse снижает debris mean на 1,07%, total
mean на 0,053%, но p99 tick выше: 21149 → 22655 µs. Это не доказательство
улучшения хвостов или FPS. Все экспериментальные изменения откатились.

## Native пары и длинный эпизод

| Ordinary 120 с | FPS | p99 frame, ms | Max frame, ms | Core/step, ms |
|---|---:|---:|---:|---:|
| Early before | 85,212 | 30,324 | 53,177 | 14,173 |
| Reuse A | 82,770 | 31,616 | 86,263 | 14,470 |
| Fresh before A | 82,255 | 31,610 | 107,408 | 14,537 |
| Before B | 81,814 | 31,400 | 50,409 | 14,424 |
| Reuse B | 77,238 | 31,982 | 462,918 | 15,179 |

Порядок A: reuse → fresh before. Порядок B: before → reuse.
Все пять ordinary runs: 3600 ticks, одинаковые gameplay lines/hash,
acceptance eligible. Pair A FPS +0,626%, core −0,461%; pair B FPS
−5,596%, core +5,235%. Reuse отклонён; устойчивого FPS gain нет.

В Reuse B rows5771..5778, ticks1496..1503: восемь тяжёлых тиков,
1890,472 мс Update + 143,588 мс following gaps = 2034,060 мс покрытия;
core 1779,146 мс. Tick1501 — 413,663 мс; следующий begin interval —
462,918 мс. Полных фаз, worker scopes и OS stacks нет: причина не установлена.
Private/working-set counters и список известных build/test names не
позволяли исключить общую нагрузку CPU или понять page faults.

## Принятый фикс наблюдаемости

`scripts/range-os-pressure.ps1` подключается только при `-OsPressure`.
Внешний sampler записывает около одного раза в секунду:

- GetSystemTimes idle/kernel/user, processor group и число processors.
  Kernel включает idle; busy считает kernel+user−idle. Разность с CPU
  всего native process доступна только для whole-machine group.
- GetProcessMemoryInfo DWORD page-fault counter, включая wrap, и
  GetPerformanceInfo available physical/commit/commit limit.
- До восьми читаемых процессов с наибольшим CPU delta за интервал.
  PID+StartedUtc разделяют lifetimes; новые и reset counters пропускаются.
  Native и sampler помечены отдельно. Число недоступных процессов сохранено.
- Стоимость чтения sensors и wall работы sampler, включая preemption.
  C# reading прогревается до sampling loop, warmup сохранён в status.

Обычный FPS режим не вызывает эти API и не перечисляет все процессы.
У native physics/render нет новых callbacks, пулов, locks или зависимостей.
Пустые счётчики остаются unavailable, counter discontinuity не выглядит
как низкая нагрузка, signed coarse differences не обрезаются до нуля.

Analyzer берёт только целые OS intervals внутри native epoch/window,
присоединяет пересечения к stall episodes и показывает границы интервала,
CPU foreign/native process и память. Он не распределяет CPU/page faults
пропорционально длительности stall. Page faults включают soft faults,
а доступная память не доказывает отсутствие paging. Защищённые и короткие
процессы могут отсутствовать. CPU delta процесса и GetSystemTimes читаются
не одновременно; это coarse observations, не OS scheduler trace.

Declared mode без actual samples и undeclared pressure artifacts invalid.
`OsPressure` — отдельный comparison flag; он всегда исключает FPS acceptance.
`compare()` сохраняет diagnostic eligibility и отдельно возвращает
`fps_acceptance_eligible`, чтобы valid diagnostics не принимать за speed gate.

## Проверка новой диагностики и камеры

Sequential native captures используют восстановленный baseline binary:

| Capture | Steps | Max frame, ms | Pressure intervals | Статус |
|---|---:|---:|---:|---|
| final-pressure-tour-120 | 3577 | 81,391 | 114 | Diagnostic valid; tempo/FPS gate false |
| final-pressure-stationary-120 | 3556 | 423,966 | 114 | Diagnostic valid; tempo/FPS gate false |

Tour: 21 segment, 134 published origins, 0 blocked frames, longest
sampled no-tick interval 33,255 мс. Автотур проверяет bases/pan/zoom;
mouse/orbit и pixel QA этим не проверены. Observer overhead есть, поэтому
результаты нельзя сравнивать по FPS с ordinary runs. Небольшой debt не скрыт.

В stationary baseline снова пойманы два длинных эпизода:

- Rows8769..8777 / ticks3014..3022: 1741,326 мс Update + 226,601 мс gaps
  = 1967,927 мс, core1620,693 мс, longest tick363,440 мс.
  Частичная phase coverage6/9; самый тяжёлый retained tick:
  debris199,786, rubble71,952, motion49,157 мс; caller CPU delta0 мс
  не означает отсутствие работы workers или доказанный lock wait.
- Rows9453..9462 / ticks3527..3536: 1725,139 мс Update + 128,281 мс gaps
  = 1853,420 мс; core1590,340 мс, longest tick317,656 мс.
  Retained phase coverage отсутствует.

Пересекающиеся coarse intervals имеют system busy до100%, при первом
эпизоде читаемый чужой `node` набрал7687,500 мс CPU за1170,792 мс wall;
во втором другой lifetime `node` набрал8484,375 мс CPU за1185,187 мс wall.
Native process в этих интервалах получил546,875 и328,125 мс CPU.
Свободная physical memory в пересечениях не ниже58,18 GB; faults высокие,
но не классифицированы. В обоих случаях выявлена внешняя нагрузка CPU;
это свидетельство конкуренции, не полное объяснение stall и не доказательство
эффекта отвергнутого reuse. Известный список build/test names показывал0
competing samples и пропускал такую нагрузку — исправлен именно этот пробел.

Whole sampler work mean33,255/35,601 мс на sample для tour/stationary;
stationary max163,086 мс включает preemption, не равен чистому observer CPU.
Sensor read max1,901/98,269 мс; null errors, no timeout, discontinuities0.
Protected/unreadable coverage явно сохранено в raw CSV.

Воспроизведение: `scripts/profile-range.ps1 -Seconds 120 -Scenarios
world-battle -SkipBuild -PhysicsWorkers 8 -NavigationWorkers 8
-ComputeWorkers 4 -OsPressure -CpuThreads -TriggerMilliseconds 40`.
Для камеры добавить `-CameraTour`. Выбирать новый output directory.
Raw: `cpu-process-samples.csv`, `cpu-os-processes.csv`, `cpu-threads.csv`,
`sampler-status.json`, native `.stall.csv`/`.camera.csv`/`.manifest.json`.
Полные интервалы и ограничения находятся в `.analysis.json/.md`.

## Проверки и следующий приоритет

116 analyzer tests и PowerShell validity/parser suite проходят, C# API
проверены отдельными process-only samples и двумя native captures.
Rejected physics candidates: final release562 passed/4 прежних ignored;
golden4, save/replay19, targeted physics49 debug passed. Первый полный debug
run остановлен из-за длительности pursuit tests; это не passed run.
Дополнительные тесты candidates не входят в итоговый production diff.

WBPERF-001/027: OS stacks/scheduler ready-wait tracing и dispatch/join
boundaries для debris/rubble; объяснить задержки при внешней нагрузке,
квантованные caller CPU не заменяют worker evidence. Pressure sampling
помогает выбрать эпизод, но не заменяет эти traces. Exhausted phase windows
и второй late stall без фаз остаются явным ограничением.

WBPERF-002: owned simulation executor с bounded completed read model,
без same-frame join. Синхронный tick всё ещё блокирует кадр; микроизменения
allocator и дробление jobs этого не снимают. Затем camera publication/repair
и strict debt (WBPERF-009), горячие direct sweeps, CPU balance и 165 FPS.
