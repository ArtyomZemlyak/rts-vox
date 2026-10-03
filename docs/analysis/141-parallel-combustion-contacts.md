# ADR-547 — общий снимок геометрии и параллельные контакты горения

03.10.2026, HEAD до изменения `6e3af20`, продолжение
[ADR-546](140-parallel-unit-poses-and-compute-pool.md), WBPERF-010/012.

## Выбор участка

Свежий CPU-профиль штатного world-battle 30×30, 3600 тиков, physics и
navigation=8: motion mean 5,003 мс, debris 3,003, rubble 1,247,
heat_flush 0,838, unit_fire 0,643. Правила ИИ около 3 мкс/тик.
Повторять отклонённый rubble pool из WBPERF-011 без нового основания
не стали: там дополнительные буферы/барьеры почти не ускоряли фазу.

У `unit_fire` нашли отдельную повторную работу. Каждая fuel collision и
каждая передача химического тепла заново измеряли все корпуса и range gun.
Несколько горящих источников использовали одну геометрию, но повторяли
построение bounds. Шесть лучей каждого источника создавали BTreeSet.

## Изменение

Границы строятся лениво один раз на фазу и используются для движения
пролившегося топлива и всех emissions. Если нет spill/огня, bounds не
создаются. Кеш локален фазе; следующий тик видит перемещения, разрушения,
смену roster и reset без version invalidation или новых сохранённых полей.

Каждый источник имеет не более шести приёмников. Array из шести Option<Site>
сортируется по прежнему Site::Ord; повторения пропускаются. Порядок точно
совпадает с BTreeSet, включая World перед Body и остаток целочисленного
деления тепла. Аллокаций дерева на источник больше нет.

От четырёх emissions и при более одного воркера контакты рассчитывает
indexed Rayon par_iter/map/collect на существующем пуле физики. Меньшие
пакеты используют serial path. Новых потоков/пулов/зависимостей нет.
Receiver selection читает геометрию, материал и принадлежность секции;
тепло, накопленное предыдущей emission, на выбор приёмника не влияет.
Списание топлива, ignition, spill movement и приём тепла остаются
последовательными. Heat capacity/admission/rejected/dissipated accounting
применяется в прежнем порядке источников; редукции в workers нет.

Скорости, энергия, квоты, урон, шаблоны, физика, частота тиков и качество
картинки не менялись. Source-level использование пула не доказывает
равномерную загрузку CPU. Новые fire worker calls пока не имеют отдельных
PhysicsTask spans: `unit_fire` остаётся измеренной parent-фазой; её wall
нельзя раздать между workers или назвать CPU utilization/lock wait.

## Проверки

Полный release sim-core: **558 passed, 4 ignored**, всего 562. Новая
дифференциальная регрессия сравнивает прежний serial BTreeSet алгоритм
с пакетным путём при 1/2/4/8 workers, пакетах 0/1/3/4/96, field limits
1/128, насыщенном тепле, нулевых emissions, повторных лучах, двух разных
receivers, непринимающем материале и gun occlusion. После удаления body
cell, перемещения gun и освобождения capacity проверяется новый снимок
геометрии. Совпадают сериализованные поля/rejected counters, точный loss;
supplied = accepted + dissipated. Прежние fire/snapshot/replay tests проходят.

После тестов visibility helper-методов сужена до private, эквивалентный
usize modulo заменён на is_multiple_of; release build/clippy проходят
на финальной версии, новых предупреждений нет. Нативные/CPU проверки
ниже выполняются её frozen бинарниками. Rustfmt/diff check проходят.
Свежий sim-cli seed42/1000ticks/5000units: hash `66041a9f60189841`.

Дополнительный harness architecture: 4 pass, **5 fail**. Те же пять
падений воспроизведены после временного восстановления двух изменённых
файлов из HEAD, с последующим точным восстановлением текущих bytes:
version pins, render writes scanner, float scanner, clock scanner, hash
scanner. Есть и старые нарушения (road generation f64, неполные pins),
и ложные срабатывания на test files/match guards. Нового общего зелёного
архитектурного gate здесь нет. Baseline log и проверка сохранены в
`out/fix547/baseline-architecture.log` / `check-baseline-architecture.py`.
Этот долг требует отдельного исправления правил сканирования и старого кода.

## CPU-пары

Один процесс за раз, RAYON_NUM_THREADS=8, `--world --ai-vs-ai --ticks 3600
--tick-csv ...`. Навигационный пул — штатный восьмипоточный. Все четыре
финальных состояния совпали: hash `bcc1f58c226d1721`, fired/damage/alive/
bases. Сравнены каждый из 3600 тиков: searching/moving/blocked/fragments/
flying/chunks/felling совпадают. CSV — диагностический CPU-стенд, не FPS.

| Run, out/fix547 | Mean tick, ms | Tick p99, ms | Unit fire mean, ms | Unit fire p99, ms |
|---|---:|---:|---:|---:|
| before | 13,523 | 22,052 | 0,643 | 1,463 |
| after | 12,964 | 21,306 | 0,215 | 0,556 |
| before-repeat | 13,502 | 22,063 | 0,641 | 1,462 |
| after-repeat | 13,006 | 21,348 | 0,217 | 0,560 |

Комбинация общего bounds snapshot, компактного receiver array и параллельных
контактов уменьшает эту фазу примерно на 66%, полный тик на 4,13% / 3,68%.
Отдельная доля выигрыша от каждого изменения не измерена.
Raw файлы `{before,after,before-repeat,after-repeat}-{bench.log,ticks.csv}`;
`bench-summary.json` содержит числа, SHA и проверку совпадения counters/state.

Frozen benchmark SHA256:
before `6d469ff44887bb7c036bf7fc8b57518f7025e592963a7f7c5a77d75b628a9376`;
after `847a37071043e0896299cda494bea19a19d53a6aa86897452395d199cb721da2`.

## Обычный полигон

Четыре последовательных runs по 120 с, без render-tasks feature,
diagnostic collectors, GPU timestamps или CPU polling. Одинаковые RON,
observed 1600×900/AutoNoVsync/камера/эффекты; compute=4, physics/navigation=8.
Во время нативных измерений нет сборок, тестов, правок или тяжёлого анализа.
Порядок: before → after → actions → after-repeat → before-repeat.

| Run, out/fix547 | Mean frame, ms | 1000/mean, FPS | Frame p99, ms | Max, ms | Mean core/step, ms |
|---|---:|---:|---:|---:|---:|
| before-native-120 | 13,2444 | 75,504 | 33,6549 | 68,0137 | 14,7748 |
| after-native-120 | 13,0329 | 76,729 | 33,7042 | 75,3419 | 14,4366 |
| before-native-repeat-120 | 13,0455 | 76,655 | 34,0875 | 85,6077 | 14,7466 |
| after-native-repeat-120 | 12,6713 | 78,918 | 33,4494 | 65,4505 | 14,3032 |

Обе strict пары проходят native/offline gate: по 3600 шагов / ~0,9995×,
hash `bcc1f58c226d1721`. Observed FPS +1,62% / +2,95%, core mean/step
−2,29% / −3,01%. Эти две пары не устанавливают статистическую значимость
или эффект для любого боя/хоста. Первая пара p99 почти прежний, max выше;
в повторе after tail ниже before. Редкие stalls не исправлены.

В первом after row5880/tick1476: один core tick 63,482 мс в Update slice
71,397 мс, следующий interval 75,342. Перед ним outside-slice gap 43,458 мс;
row6332/tick1743 core60,651. В ordinary run нет подробных фаз/OS stacks,
причину нельзя объявить новым pool, GPU, fire или mutex. Требуется trigger
и source-aligned child/OS evidence; средний FPS эти эпизоды не объясняет.

`actions-20`: reset generations 1…4, 455 сценарных шагов; gate проходит.
Mean 2,710 / p99 8,285 / max45,295 мс; паузы/сбросы сценарные, это
не realtime world-battle acceptance. Hash `746216a0191f1c9e`.

Frozen ordinary SHA256:
before `7c9080ce252c13fe254003882ec51026e59728233adfe290e520996a5876e9c4`;
after `422a9a556f22141e232acdf33dafc2afc10f3b12675e11eabe2c1d2202d714de`.
Оба exe содержат одинаковую незакоммиченную ADR-523 работу; это не чистые
Git snapshots. HEAD в manifest не доказывает build source identity.
Все raw CSV, manifests, logs и offline reports — в названных директориях.

## Далее

165 FPS не достигнуты. Более дорогие motion/debris/rubble, mesh instance
queues и последовательный core на пути render frame остаются основными
задачами. WBPERF-002/027 — независимый sim/render и причины длинных пауз;
WBPERF-001 — child workers для нового fire batch и mesh producers с source
IDs, без приписывания всей parent wall одному CPU. WBPERF-010/012 частичны.
