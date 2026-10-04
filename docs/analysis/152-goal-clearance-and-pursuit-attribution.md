# ADR-558 — дешёвая проверка цели и наблюдаемое перестроение маршрута

04.10.2026, исходный HEAD `41762f5`. WBPERF-001/027 и навигация частично.
Артефакты: `out/fix558`. Сборки/тесты/анализ выполнялись между измерениями,
не одновременно с ними. Все прежние 13 WIP entries остаются вне commit.

## Причина пика и изменение

Два больших orders_misc предыдущего ADR — непосредственные pursuit retarget,
не muzzle/laser или выбор позиции. Первая native trace дала в tick 1606
два orders_retarget по 10,124 и 9,344 мс. Firing point/flank — микросекунды.
Headless detail отделил coarse, target ring, sweep preparation и direct run.
Отдельные target poses занимали 5,844/5,787 мс, а не сотни дешёвых кандидатов.

При выборе допустимой точки Hunt читает только Pose::refused. Стоимость
сноса деревьев нужна для оценки маршрута, но этот вызов её не использует.
Goal query теперь считает точные контакты/уклон/наклон и проходимость,
используя prepared non-yielding floor upper. Yielding canopy не мешает
такому доказательству. В обычной route/motion geometry crush остаётся.

Rubble::upper_in даёт точный максимум внутри X/Z box, с прежней sparse/dense
стратегией strips. Верх debris ниже минимальной belly plane не требует
точного footprint scan. Bare proofs полного Pose по-прежнему исключают
yielding matter; floor-only proof применяется только к goal refusal.
Неподготовленные/изменённые/несовпадающие material-profile tiles идут
в прежние точные чтения. Cache invalidation мира не менялась.

Нет новой зависимости, баланса, упрощения коллизии, увеличения бюджета,
сдвига authoritative edits, смены порядка/числа logical goal probes,
full-world copy или новых pools. Physics/navigation parallel work сохраняется.
Число физических чтений/кэш-попаданий изменяется; это диагностика,
не дополнительная разрешённая работа поиска.

## Наблюдаемость

Battle pursuit emits orders_approach/firing_point/flank/retarget_setup.
Retarget forwarding отделяет orders_retarget_coarse/target/prepare/direct/fine.
Последний orders_retarget — остаток после последних вложенных границ,
не inclusive parent. Эти completed intervals составляют последовательную
wall partition и не суммируются ещё раз как вложенные parent durations.
Bench TickProfile включает все orders_* в battle_orders_us.

`bench_battle --order-trace-tick 1606` печатает длительности фаз одного
выбранного тика. Это diagnostic IO; для timing runs этот флаг не применяется.
Массовые native collectors и их opt-in режимы прежние. Unit identity внутри
плоских phase boundaries пока нет; OS waits/CPU utilization они не доказывают.

## Headless и diagnostic evidence

Headless 1800 ticks, world/AI обеих сторон, physics/nav=8, before detailed
и goal-clearance: hash `bdfb97cef98d13e2`, полные финальные gameplay counters
совпадают. Tick 1606: 50,941→38,044 мс; goal phase p99/max 7,372→0,916 мс.
Mean tick 10,510→10,328 мс — один инструментированный CPU-проход каждой
версии, не доказательство общего FPS gain. Повторных CPU timing пар нет.

Matching flags native diagnostics (CorePhases, без GPU/RenderTimeline),
1800 steps, tempo 1×, gates/clock/physics coverage проходят.
Before attribution имеет менее подробные labels внутри retarget; after —
новые subphases, поэтому суммы одноимённых retarget labels не сравниваются.
Полный tick 1606: 51,745→40,229 мс; after goal p99 0,900, max 0,913 мс.
Observer overhead присутствует, diagnostic FPS не используется для приёмки.

After восемь navigation OS IDs и восемь physics worker IDs, плюс main caller
physics scopes. Единственный tick caller остаётся main thread (ADR-557).
Это факт исполнения jobs, не доказательство равномерной загрузки всех CPU.
Подготовка direct sweep tick 1606 остаётся 4,033 + 3,447 мс.

## Ordinary native: выигрыш хвоста, небольшой проигрыш среднего FPS

Четыре последовательных runs по 120 с: before→after→after→before.
60 units, AI обеих сторон, physics/nav=8, compute=4, 1600×900 AutoNoVsync,
обычные effects, detailed collectors выключены. Каждый run: 3600 steps,
tempo 1×, evidence/FPS comparison gates проходят. Все gameplay lines
(shot/damage/alive/streams/fuel) и hash `bcc1f58c226d1721` совпали.

| Метрика | До / 1 | После / 1 | До / 2 | После / 2 |
|---|---:|---:|---:|---:|
| FPS | 85.519341 | 84.787347 | 84.969342 | 84.187023 |
| Mean frame, мс | 11.693261 | 11.794213 | 11.768951 | 11.878315 |
| p99 frame, мс | 30.480500 | 30.635200 | 30.929000 | 30.871700 |
| Max frame, мс | 65.501700 | 56.893900 | 63.201200 | 53.896400 |
| Core call/step, мс | 14.285990 | 14.257506 | 14.303352 | 14.225115 |

FPS **−0,856% / −0,921%**, а не выигрыш. Core call/step −0,199% / −0,547%.
Max frame ниже на 13,14% / 14,72%; p99 первого run немного хуже, второго
почти прежний. Принято сокращение конкретного большого serial hitch и
его ясная attribution с открытым небольшим tradeoff среднего FPS.
Это не успешная общая FPS оптимизация; общий scheduler/render критический
путь и 165 FPS остаются открыты. Process core-equivalents около трёх;
увеличение/баланс CPU utilization не установлены. Следующий этап WBPERF-002
важнее наращивания jobs маленьких проб.

## Отвергнутые варианты

- Goal batches до 32 poses, par_chunks(4) в existing pool. Cheap prefix
  сохранялся, но expensive случай — первая допустимая отдельная pose.
  Goal p99 7,372→7,394 мс; результата для проблемного query нет. Откат.
- Только rubble upper без отделения crush: goal p99 7,636 мс.
  Это само по себе не решило target spike. Helper сохранён в конечном
  доказательстве bounds; isolated speedup не заявляется.
- Первые четыре sweep poses параллельно (four leaves) и ordered join,
  last fork cache принят для reuse. Sweep prepare p99 4,122→4,253 мс,
  mean 110→110 мкс; выигрыша нет. Полностью откат, adopt API удалён.
  Intermediate вариант без сохранения cache не измерялся. Попытка
  восстановить Probe из захваченного world/rubble не прошла HRTB lifetimes;
  временный adopt helper использовался только в проверенном prototype.
- Temporary per-pose marker обнаружил два крупных одиночных запроса;
  первоначальный bench consumer не знал marker и упал. Consumer для
  exploratory run исправлен, затем marker и его case удалены из final.

Rejected source/binary/build manifests/logs сохранены в out, в main их нет.
После restores final refresh/hash/freeze protocol ADR-554 пересобрал range.

## Проверки и пределы

- Full release voxel/nav/sim-core lib: **156 + 104 + 562 passed**, 4 прежних
  ignored. Новый тест сравнивает goal refusal с exact scan (reverse profile
  отключает prepared proof), forests, dense heaps, tilt, live hard edits;
  отдельный тест сохраняет full route vegetation cost и снижает column work.
- Rubble upper vs exhaustive rectangle scan: sparse/dense strips, edges,
  negative coordinates, empty/reversed boxes.
- Pursuit observations/worker override сохраняют state hash каждого тика.
  Большие serial/parallel battle/replay tests из полной suite тоже проходят.
- Bench aggregator **1 passed**; полигон **118 passed / 3 прежних ignored**.
  Итого 941 passed / 7 ignored. Clippy проходит с прежними warnings,
  новые строки warnings не добавили. Own formatting/diff checks проходят.
- Первый новый forest fixture не применил всю deferred edit queue: его
  cardinality assertion упал. Fixture flushes queue полностью, после
  исправления три новых geometry tests и полная suite проходят.
- Native reset/actions в этом шаге не запускались; UI/lifecycle покрыты
  существующими render tests. Preview/video не снимались.
- Source/build attestation относится к перечисленным refreshed inputs;
  baseline reused verified ADR-557 coordinator, не clean checkout closure.
  Все WIP в sim/weapon/UI/data/docs сохранены вне commit.

Final/current range SHA256:
`c7349e72bacb55b14d1c4a14ec66234db4ba0f8b56e204d5c1fbfff64b9e3015`.
Before:
`f59b60a22f17fc3fa08824c63aeb2be0e49ce3545ab42a6c85f8f398e9f4016a`.
Отчёты/полные OS IDs: native-summary.json, diagnostic-summary.json,
frozen manifests, четыре ordinary runs и final diagnostic.
