# 650 — Round4: wave capture tail и Spider abyss goal

2026-11-18. База649 `49462b4bddb05ca7678191b1e946454e0f88d930872fcb8958ec3ad1c2312c85`.
Отдельная pure11-фаза `9618c0719a35a370a038211a7f98152f3c6f8c76a50b02f0bb0f4af9cbd1b181`.
Финальный compile source `390b2196c0803b893814f4211741ad94c3fdde94bee280b98c7f9569707411da`; development binary `out/round650/blast-round650.exe`,
SHA256 `f8d39deab89060c897eff61a0ef83fd817c3b55b3650863d9c210eb0778bf29a`. Это механический кандидат, не общая доставка627.
Входные SHA256 patch11/12 сверены с сообщением; старые isolated receipts не выданы
за текущий main. UI06 уже эквивалентен647, повторных изменений construction нет.

## 11: текущий review и точность capture

Построчный uniform_box допускается только без пересечения frozen body bounds
и без dents в строке; частичные строки, sphere mask и границы мира сохраняются.
terrain_work монотонно растёт с глубиной для1/7/8, прочие материалы постоянны:
нижний включённыйY даёт точный максимум однородного ряда. Неизвестный сертификат
оставляет точный reader. Для cell2..64 включены прежние packed columns; dented
содержит только уже зафиксированный slice_end диапазон. Quota/FIFO/материя,
frozen poses/live body damage и порядок редукции не менялись.

Шесть существующих capture-oracles дополнены независимым dense-voxel тестом:
edge2/3/4/5, soil depth, homogeneous stone/soil, AIR/outside/bottom, partial-row
budgets, dents и правка будущей клетки, workers1/2/4/8. Он считает ожидаемый max
не через новую dent-map и требует исполнения многоклеточного uniform-row proof.
Все7 прошли отдельно на11 и на финальном source.

Unchanged bench_battle,300ticks,nav2/physics2,CPU8/Rayon2: before11/after11
сохранили все не-временные counters, command CSV и terminal hash
`7c025485ae1c180c`. Для final12 тот же300tick результат тоже совпал; это не
заявление о неизменности12 на других входах. Полные state_hash каждого тика
между версиями бой-driver не записывает; отдельные workers/cold тесты сохранены.

| Capture,33calls | До11 | После11 |
|---|---:|---:|
| mean per call,мс |12.782|7.945|
| p95,мс |28.339|16.964|
| max,мс |34.621|22.365|
| mean per simulation tick,мс |1.406|0.874|
| весь тик mean,мс |40.633|38.899|
| весь тик p99/max,мс |111.633/129.955|105.205/115.195|

Одна актуальная пара, не обычный FPS. Фазы пересекаются, складывать нельзя.
Existing explicit manual benchmark выполнен отдельно в sim-test, не release:
packed/scalar columns сохраняют serialized bytes; budget65536 median15.000→7.287мс,
full21.519→10.851мс. При4096 packed0.909 против scalar0.807мс: маленький срез
не ускорен. Это сравнение двух путей текущего11, не новый frozen-baseline A/B.
Измерительный #[ignore] явно запущен с --ignored:1passed/0ignored; не скрытый skip.

## 12: цель в пропасти и фактический контакт

Будущая self.goal ограничена lane/reach envelope только при lock_anchor,
ниже нейтрали, вне досягаемости и при вертикальном дефиците>установленного
g.lift. Может корректироваться X/Z будущей цели вместе сY; это не перенос
planted anchor или from. Верхняя преграда/обычная глубина/unlocked путь не
получают эту коррекцию. Actual touchdown заново спрашивает поверхность;
Pending не превращается в Ready из-за геометрического ограничения.

Добавлен trace-only GoalEnvelope (не save field): before/after — будущие цели,
planted_before/after=false. Две дополнительные проверки и расширение входного
теста держат anchor/from, lock/lift/direction, save/load и не скрывают настоящий
unreachable departing-origin. Старые arriving/departing и retry regressions
также прошли:5focused. Runtime/render-only обхода или смещения готовой позы нет.

До12 четыре620tick манёвра реально повторены на11 и совпали с648. После12:
cold restore41/301, W1/4/8 против W2, hash каждого последующего тика совпал.
Начальные snapshots и весь recording кроме expected-final-hash/checksum
побайтово одинаковы. Во всех4сценах maxIK=0мм, planted/Ready=0мм, up/reversed=0.
Нольмм — разрешение отчёта, не обещание математического raw-Fx нуля.

| Scene | maxIK,мм | путь,мм | поворот,° | max drop за тик,мм |
|---|---:|---:|---:|---:|
| 2 | 0 → 0 | 32729 → 32729 | 713 → 713 | 199 → 199 |
| 3 | 532 → 0 | 16733 → 16693 | 403 → 407 | 363 → 128 |
| 16 | 0 → 0 | 16550 → 16550 | 237 → 237 | 621 → 214 |
| 22 | 0 → 0 | 62259 → 62259 | 1578 → 1578 | 170 → 170 |

Наш scene22 имеет62259мм, не61184мм из isolated отчёта; чужая база не подставлена.
Scene2/22 сохранили весь pose CSV и terminal hash, поэтому нативные кадры для них
повторно не снимались. Scene16 сохранил путь, но изменил сохранённую цель/hash:
он проверен заново вместе со scene3. Два native replay прошли все620ticks на
финальном binary, с ожидаемыми hashes. 18companionJPEG и9targeted previews
осмотрены; две38s realtime1× записи закодированы/декодированы. В них31sSIM и
хвост после replay, поэтому это не доказательство FPS боя. PNG сначала проходили
image-preview.py; исходные PNG/пользовательское вложение не менялись.

Bounded trace:cap32/тик, максимум128 GoalEnvelope rows;2события на474/491,
leg6, dropped0. Traced результат и cold hashes равны untraced. Прежний
scene3 tick493 leg6 q0.48935 532мм закрыт на реальном манёвре. Историческая
двухногая поза пользовательского screenshot точно не воспроизведена, поэтому
весь623 не объявлен закрытым. Большие вертикальные переходы корпуса остаются
отдельными свидетелями; IK0 их сам по себе не исправляет.

## Текущая приёмка и боевой FPS

На одном финальном source:7capture,5focused,125suites (включая craters,
walking-blasts/supports/architecture/docs),83movement — passed,skip/failed/ignored0.
Завершающий docs receipt после отчёта хранится отдельно. Development build40.224s
успешен. Предыдущие Scalar oracle и save/restore/workers не ослаблены.

Четыре настоящих background20s боя:1080before→after,4Kafter→before;
baseline frozen649, candidate combined11+12, workers2/CPU8/Rayon2/cap200,
одинаковые input hashes. Общий tick-window[60, 290], workload
signatures equal=True. Fighting/shots/damage и
нагрузка мира/обломков подтверждены. Encoding/build/tests закончились до замеров.

| Режим | FPS | p50/p95/p99/max кадра,мс | SIM/20Hz | max debt/dropped,ticks | private/working GiB |
|---|---:|---:|---:|---:|---:|
| 1080-after | 13.425 | 70.18/119.66/150.51/167.96 | 0.671 | 19.00/92.96 | 18.48/15.79 |
| 1080-before | 14.664 | 65.96/113.77/140.88/154.67 | 0.697 | 19.00/80.40 | 18.51/15.83 |
| 4k-after | 13.837 | 70.82/112.13/133.22/148.15 | 0.692 | 19.00/82.78 | 18.80/15.86 |
| 4k-before | 13.653 | 70.51/116.64/144.68/157.70 | 0.683 | 19.00/87.39 | 18.86/15.87 |

Первая1080пара отрицательная:14,664→13,425FPS;4K13,653→13,837.
Выполнен один обратный контроль1080after→before без сборок/encoding/тестов.
ABBA на общем окне60–290:before14,166FPS,SIM0,690×;after13,655FPS,SIM0,683×.
Среднее p99 двух run-quantiles142,98→140,66мс,но средний FPS−3,6%; общий
performance/no-regression допуск не выдан. Workload signatures всех4runs совпали.

В исходном after1080 tick132 motion_feet26,4457мс: caller CPU counter0мс,
93310cycles; feet unit46 wall25,8889мс/2940340cycles и unit22 wall22,9071мс/
1923390cycles. CPU time имеет грубое OS-разрешение: нельзя назвать эту wall
величину25мс активного IK. Счётчики указывают преимущественно на ожидание/снятие
сCPU, но точный источник не установлен. Остальные nativefeet maxima2,6–2,9мс.
Capture mean/call в native1080/4K13,254/13,406→8,445/8,255мс:локальный tail
выигрыш повторяется,общий FPS этим не исправлен. Данные:
repeat1080-summary.json и feet-tail-inspection.json; первый плохой run сохранён.

Это background diagnostic одного малого боя64→73, не ordinary/release/5000-unit
допуск. Полные peaks units/moving/waves/fragments/felling, memory1Hz (неVRAM),
debt и перекрывающиеся core phases — battle-summary.json. Реальное время,
масштаб128/1000/5000 и завершённые приказы625 этим не доказаны.

## Done / remaining

| Работа | Статус |
|---|---|
| 11 capture | done:текущие oracles/exact A/B и сниженный capture tail |
| 12 известный532мм | done:все4scans IK0,защита takeoff/Ready,два полных native |
| Историческая поза623 целиком | remaining:точная игровая последовательность неизвестна; не закрыта красивым кадром |
| Root vertical transitions | remaining:см.table и прежний648followup; не маскировать только визуально |
| Felling seed coalescence628 | remaining:получена диагностика, не готовый patch; origin/radius/FIFO ещё требуют отдельной работы |
| 625/627/ordinaryFPS/release/доставка | remaining:общий admission и реальное время не выданы |

Changed compile inputs только range_capture.rs/gait.rs. UI647, nav649,
physical foliage, SOLID_SAMPLES64 и все предыдущие незакоммиченные работы
сохранены. Не добавлены зависимости/новый физический мир/искусственный sink.
Release не пересобирался,«Полигон»627M не переключался; canonical SHA256
967bdacc50aff659a9f485808edda7e0c73c0ca5eb36ddd263efd5faea989032.
В walking648 followup отмечено закрытие конкретного532мм; original/root-drop
работы сохранены без изменения app queue. Все собственные процессы завершены,
фоновая работа не обещана. Итог:out/round650/final-evidence.json и ledger.json.
