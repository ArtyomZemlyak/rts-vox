# Единый план производительности движка

Актуализация: **19.10.2026**, база `main 0387466`, срез334. Это **единственная рабочая
очередь оптимизации**. Номера OPT316 сохранены. Здесь находятся статусы,
зависимости и приёмка; измерения и архитектурное обоснование —
[сводный аудит321](321-engine-performance-review.md).
Исторические планы не обновляются копиями новых результатов.

## Цель и границы

Бюджеты определяет [канон10](../10-budgets.md): headless бой — ≤33 мс
**каждый** тик; 30 Гц без списания времени; 165 FPS/≤6,06 мс каждый кадр
в заданной матрице полигона. Большой World проверяется отдельно.
120 FPS/p99≤8,33 мс — промежуточная диагностика, не новый норматив.
На текущем срезе эти цели полностью **не достигнуты**.

- Общий движок обслуживает все сценарии. Сценарий задаёт расстановку, ИИ,
  задачи и команды; не выключает физику специальной веткой.
- Сохраняются сила и радиус оружия, геометрическое укрытие, броня, материя,
  same-tick collision/navigation, причинный порядок и частота SIM.
- Точная оптимизация сохраняет logical quotas, сроки, состояние и replay.
  Смена физической модели/расписания — отдельное решение с последствиями.
  Новые направления ниже пока не принятые ADR.
- Дешёвый тик с растущей очередью, запоздавшей воронкой или потерянным
  приказом не считается успехом. Скорость расчёта, обслуживания работы
  и появления её результата на экране проверяются раздельно.

## Текущее состояние и очередь

Базовый аудит321:6000 шагов, mean18,088, p9944,849, max106,125 мс.
Поздний mean20,755 мс: rubble7,143, debris5,364, motion4,720 мс;
три **непересекающиеся** фазы составляют83,0%. Это основание порядка ниже,
не прогноз ускорения. SIM всё ещё синхронно вызывается из Main Update.
Проверенный срез322 сокращает late columns примерно на30%, late rubble
на13–14%, весь mean SIM на4,5–5% в двух парных6000-контролях. Межтиковый
индекс ещё не реализован; max шага остаётся около106 мс.

Срез323 распределяет независимые запросы поддержки по существующему пулу:
late supports примерно на71% дешевле, весь mean SIM на2,6–3,1% в повторных
диагностических контролях. Ordinary пара подтверждает mean−2,5%, P99
43,492→43,037 мс; в подробном профиле P99, напротив, выше. Every-tick gate
не пройден. Late rubble теперь5,34–5,46, debris5,22–5,32, motion4,70–4,73 мс:
два первых расхода сопоставимы. Полная таблица и пределы — [323](323-rubble-support-queries.md).
Ordinary native240 среза323:3840×1949,39,7 FPS,p9956,54 мс,
6926 тиков,ratio0,96159; per-run evidence valid, FPS acceptance false.
Это одиночный after, не квалифицированный FPS A/B.

| ID | Приоритет / статус | Следующий проверяемый результат | Зависимость |
|---|---|---|---|
| OPT316-00 | P0, базовые часы готовы; пробелы открыты | Узкие счётчики причин/стоимости и сопоставление CPU/GPU кадров | Перед своим экспериментом |
| OPT316-01 | **Выполнен**,317 / ADR-743 | Равный rubble сохраняет coarse cache | Только регрессии |
| OPT316-02a | **Выполнен**,318 / ADR-744 | Ограничена лишняя подготовка coarse cells; общий peak gate открыт | Только регрессии |
| OPT316-02b | **Частично**,319/320 / ADR-745/746 | Columns готовы; дальше exact bulk apply и summaries | Профиль apply/claims |
| OPT316-03 | P0, **открыт**, drain321 выполнен | Полнота контактов, причины удержания slots, меньше повторов, устойчивый progress валки |00; не блокирует точный04a |
| OPT316-04, подготовка колонок | **Выполненный срез322**, ADR-747 | Компактные footprint spans и прямые максимумы строк; повторные клетки не материализуются | Прежние проверки опор и fallback |
| OPT316-04, чтение опор | **Выполненный срез323**, ADR-748 | Независимые supports на замороженном индексе; ordered join | Полный rebuild индекса сохранён |
| OPT316-04b, точные строки | **Выполненный срез327**, ADR-749 | Переиспользование maxima при полном равенстве свежих входов; supports по-прежнему свежие | Полный oracle и fallback |
| OPT316-04, сборка списков | **Выполненный срез334**, ADR-751 | Один head вместо head/tail; обратная вставка сохраняет source order и wide links | Полный rebuild сохранён; qualification ниже |
| OPT316-04a | P0, **следующий основной SIM-кандидат** | Сохраняемые bounds/membership resting fragments |00, полная invalidation |
| OPT316-04b | P1, **частично**, остальное открыто | Опоры и rubble columns по изменившимся зависимостям |04a, full oracle |
| OPT316-05a, ключи слияния | **Выполненный срез332**, ADR-750 | Точный raw-shift для power-of-two pile_cell вместо runtime divisions | Полная прежняя физика/порядок |
| OPT316-05a | P1, **открыт** | Active/dirty IDs debris вместо обслуживания всей истории |04a либо отдельный доказанный индекс |
| OPT316-05b | P1, **открыт** | Активные позы, dirty geometry, стабильные render slots/meshes | Профиль producers |
| OPT316-06 | P1, **открыт** | Terrain query reuse, локальные соседи, latency группы | Motion baseline |
| OPT316-07 | P1, **открыт**, отдельная ось полного экрана | Gas skipping/композиция, освещение и terrain | GPU attribution, визуальный эталон |
| OPT316-08a | P1, **открыт**, контракт готовить сейчас | Полная owned публикация | Inventory readers, версии, цена экспорта |
| OPT316-08b | P1, **открыт** | Единственный SIM owner вне Main Update |08a, bounded lifecycle |
| OPT316-09 | P0 при приёмке каждой серии, **открыт** | Ordinary native, очередь, память, долгий бой, все сценарии | Каждый принятый срез |
| OPT316-10 | Следующий этап масштаба, **открыт** | Общий backend, экономика, реальные100/500/1000/5000 | Roadmap, оставшиеся механики |

Порядок: **00 по месту →04a + диагностика03 →04b/05a**; пики02b и
контракты08a независимы. Затем05b/06/07 по повторному профилю,08b после
готовности границы. «+» означает независимость задач, не одновременные
benchmark. Полный rewrite опор не начинается до узкого04a. Подтверждённая
потеря игровых контактов получает приоритет над gain.

Срез327 добавляет узкое точное переиспользование строк после полной свежей
классификации. В двух подробных контролях late columns дешевле16–17%,
rubble4–5%; root во второй паре без выигрыша. Итоговый ordinary6000:mean17,266→
17,062 мс, late rubble5,598→5,318; P99 root45,816→44,692 мс,
max105,860→108,409. Ещё пять сцен дают root−4–6% при равной работе.
Это локальное ускорение, не исправление пиков или выполнение30Гц/165FPS.
Persistent membership04a и остальные зависимости04b не реализованы.
Native240:3840×1949,38,5 FPS,p9957,881 мс,6871 шаг,ratio0,95397;
evidence valid,FPS acceptance false. Свежий reverse native baseline даёт38,6 FPS
и6866 шагов. Общие tick1..6000:core19,799→19,669 мс,13 игровых полей
совпали; прирост FPS и улучшение хвостов не доказаны. Это не matched FPS A/B.
Подробности и пределы — [327](327-exact-rubble-row-reuse.md).
Отвергнуты [324](324-hull-support-candidates.md),
[325](325-settled-column-heads.md), [326](326-hull-roof-levels.md):
улучшение внутренних кандидатов не ускорило весь тик; runtime прототипы удалены.

Дополнительные пробы328–331 не приняты: [компактные посадки328](328-compact-fragment-landings.md),
[узкие endpoints329](329-compact-settled-endpoints.md),
[полный skip mask330](330-complete-hull-skip-mask.md),
[rigid contact geometry331](331-rigid-fragment-contact-geometry.md).
Одна первая пара328 имела меньший root, но targeted translation не ускорилась;
в остальных root дороже. Все варианты сохранили36 игровых полей и hash,
но этого недостаточно для performance gate. Runtime прототипы удалены;
полные/native проверки не приписываются адресным debug и probe runs.

Срез332 сокращает подготовку pile keys на20–23% в двух6000-контролях.
Средний root по двум before/after16,981→16,939 мс (−0,24%); свежая
соседняя пара17,030→16,886 (−0,85%), первая без root gain.
Это небольшая арифметическая оптимизация, не закрытие active IDs05a,
persistent membership04a, редких пиков или30Гц/165FPS.
Материя, все36 игровых полей и hash прежние. Подробности —
[332](332-pile-grid-integer-division.md).
Итоговый binary отдельно проверен6000 шагами:keys0,215 мс,36 полей/hash
совпали. Пять сцен:root−0,4–1,4%; full release1011 passed,16 ignored.
Native240:3840×1949,40 FPS,p9957,162 мс,6917 шагов,ratio0,96029;
per-run evidence valid,FPS acceptance false. Это одиночный after,
причинный FPS gain не заявляется.

Срез334 сокращает повторные записи при сборке списков опор: один head
на клетку, обратная вставка сохраняет прежний source order. Две свежие
соседние ordinary6000 пары b/c: late cells−15,1%, Settled−7,2%,
debris−4,3%, root16,97076→16,69434 мс (−1,63%). Ранняя пара a почти
не ускорила cells и ухудшила root; с ней средний root gain лишь0,51%.
Все36 игровых полей/hash совпали. Пики,30Гц/165FPS и persistent
membership04a остаются открыты. Полная приёмка и ограничения —
[334](334-settled-head-only-lists.md). Это локальное ускорение builder,
а не15% всего движка. Перед ним отклонён [333](333-equal-settled-request-runs.md):
группировка одинаковых requests удорожила cells и root; runtime удалён.
Полный release1011 passed,16 ignored; пять дополнительных парных сцен
сохраняют36 игровых полей/hash, root от−0,84% до+0,49%.
Final binary отдельно проверен6000 шагами:mean16,436,p9946,428,max107,122мс;
это qualification, не новая A/B. Native240:3840×1949,40,2FPS,
p9956,249,max125,043мс,6948steps,ratio0,96465. Per-run evidence valid,
FPS/realtime acceptance false; причинный прирост FPS не заявлен.


## OPT316-00. Наблюдаемость по потребности

Существующие инструменты сохраняются. Добавляются только недостающие
наблюдения для выбранной гипотезы:

| Для задачи | Добавить / проверить | Что различать |
|---|---|---|
|03| Возраст разных записей, владельцы slots, позиции/seeds, полный captured contact против prefix | Потеря контакта, starvation, медленный drain, дорогая обработка |
|04/05| Dirty providers, candidates, hit/invalidation/fallback, poses/geometry, bytes/capacity | Вся цена индекса, не только query |
|02| Read/consumed/discarded subcells, bytes, material/claim work, join/apply | Реальная работа на прежнюю logical quota |
|05b/07/08| Moving/fragments/bodies отдельно, mesh producers, GPU source frame/tick/generation и coverage | CPU, GPU, surface wait, публикация |
|09/10| Heap owners, live GPU bytes, cold start/save/load/reset, economy/placement по числу линий | Payload, retained capacity, драйвер и масштаб |

Часы находятся в стендах/рендере и не управляют SIM. Счётчики не обходят
миллионы записей каждый кадр ради HUD. Observer overhead измеряется;
подробная трасса не выдаётся за ordinary FPS.

## OPT316-04. Главный средний расход: история опор

**Срез322:** профиль отдельно выделил columns как крупнейший parent внутри
rubble. Первая проба persistent local bounds не ускорила classify и удалена.
Внедрена подготовка опорной карты компактными footprint-диапазонами и прямыми
максимумами строк, без промежуточного списка повторных клеток. Это точный
общий алгоритм одной фазы; полный пересчёт опор остаётся. Замеры, проверки
и ограничения — [322](322-rubble-column-spans.md).04a/04b не закрыты;
следующий persistent-кандидат должен измерять bounds **и membership/queries**,
учитывая отрицательный результат простого sidecar cache.

**Срез323:** независимые `supports` читают один замороженный индекс
indexed пачками с объединением в исходном порядке. Малые наборы и один
поток сохраняют последовательный фильтр. Это отдельное ускорение чтения,
не persistent membership и не событийные ответы. Проверки и замеры —
[323](323-rubble-support-queries.md).04a/04b остаются открыты.

**04a — индекс геометрии.** Места: `burst::rubble_with_scratch`, `Settled`,
`step_fragments_kernel`, `BlastRange::refresh_rubble`. Буферы уже повторно
используются, ответы и membership пока строятся заново.

1. Derived index: stable FragmentId, bounds, geometry/pose/rest revisions.
   Авторитетный массив и порядок ID прежние.
2. Покрыть spawn/move/rest/wake/push/evict/merge/reclaim/bake/sections,
   изменения тела, материала, настроек. Load/reset строят холодный индекс.
3. Сначала переиспользовать геометрию/членство, сохранить алгоритм опоры.
4. Измерять все обновления, память и fallback; при dense dirty полный
   builder может быть дешевле. Новый индекс не обязан выигрывать везде.

**04b — ответы опор и колонки.** Версии terrain edits, old/new/swept hull
footprints, carried pieces, обратные зависимости через цепочки опор.
Граница чанка не обрывает каскад. Колонка хранит вклады: удаление максимума
должно находить следующий. Малая итоговая дельта высот не доказывает малую
работу. Снимок начала motion нельзя смешать с поздним снимком debris.

**Приёмка:** full builder и candidate равны на каждом фазовом входе:
contacts/rubble/paths/state bytes; подкоп, крыша, отъезд танка, evict за
исходный sweep, merge, тонкие плиты, restore и команды между тиками.
Late World быстрее в повторных A/B; cold/dense fallback и память проверены.
Не пропускать проверки по одному `max_flying == 0`: контрпример283.

## OPT316-03. Валка: обслуживание и полнота

Drain321 после принудительного Hold/Setup без новых выстрелов:
за1800 шагов очередь5,175→1,553 млн; физика породила ещё3,091 млн пар.
Это диагностический fixture, не обычная пауза и не завершённый drain.
Контакты от пассивной посадки корпусов этим не исключены.

- Разделить contact capture, пары `(position, seed)`, claims/reserved slots,
  lifetime flying pieces и propagation arrivals.
- Проверить полный capture траектории всех пяти подвижных chassis и обеих
  сторон: full pool, отъезд, save/load. EP-06 full capture остаётся открыт.
- Точный срез: совместная подготовка одинаковых material/claim queries
  внутри immutable фазы; seed identity, порядок и оплата visits прежние.
- Продлить drain до завершения/объяснённого plateau; измерять возраст
  классов запросов. Один head-age или уменьшающийся общий Q недостаточен.
- Если причина в fan-out/admission, отдельно проектировать connected
  vegetation/pieces или fairness с ADR, геометрией/ledger/сроками/golden.

**Приёмка:** контакты не теряются, растение падает после отъезда, материал
сохранён, цена прежнего потока меньше. Устойчивость закрывается доказанным
progress/drain; дешёвые отказы и поднятый max_flying её не доказывают.

## OPT316-02b. Крупные взрывы: объём и commit

Сделанные318–320 изменения не переделывать. На fixed tick2227 разложить
apply на material/energy, claim membership/connectivity, edits/journal
и ownership. Проверить homogeneous span/chunk summaries и bulk masks
через общий voxel API, без среднего сопротивления вместо максимального.
Каждый dent, thin armour, shell/bedrock, partial cells и неполная энергия
требуют exact fallback. Пакетировать независимое чтение/арифметику;
конфликтующие claims/slots и снятая материя принимаются в прежнем порядке.

**Уточнение321:** `range_battle` уже параллелит capture **между волнами**,
затем frontier+read **между волнами**, затем выполняет ordered apply.
Просто «параллелить волны» исключено как дубликат. Low-confidence опыт:
перекрытие capture и frontier **разных стадий** после доказательства тех
же slices/budgets и read snapshot. Память временных результатов, nested
parallelism и неудачный опыт241 требуют нового измеримого обоснования.
Это вторично после apply, не обещание ускорения.

**Приёмка:** identical contacts/snapshot/energy/ledger на1/4/8, edges1–16,
air/dense/mixed, несколько волн и full pool. Быстрее пик и root; отдельно
crater→collapse latency. Радиус и сила T3 прежние. Ещё большая клетка
в центре меняет occlusion и ground subcells: бесплатным упрощением не является.

## OPT316-05–06. Активность, представление и движение

**05a:** active/dirty IDs, локальные wake candidates и reuse допустимого
фазового индекса. Сохранить stable IDs, due/deadline, порядок capacity
awards, landing/crush/merge/hull riders и save/reclaim холодной истории.
Измерять touched/retained, а не только число flying.

**05b:** сначала разделить moving/fragments/bodies и источники mesh churn.
Dirty geometry и активные позы со стабильными slots; затем instancing
спокойных кусков с прежними picking/damage/shadows/material/retirement ID.
Picture pool уже проигрывал154: повтор без новой причины исключён.

**06:** каждый корпус сейчас собирает `others` из всего roster.
Spatial broad phase должен обновляться после каждого accepted pose,
сохраняя tie-break. Нынешний `other < id ? other : other + 1` предполагает
полный список: локальным кандидатам нужен явный stable actor ID.
Terrain query reuse по footprint/версии и общие коридоры испытывать отдельно;
один путь для разных габаритов без проверки недопустим.

**Приёмка:** replay/hash,10/16/60 юнитов вместе, встречные группы, разные
габариты, проход/кратер; command→ack→поворот→смещение, route latency в
тиках и wall, отсутствие застреваний. Историческое C5 max100 тиков для
определённого группового fixture остаётся открытым; другой состав его не закрывает.

## OPT316-07. GPU полного окна

Кандидат — экранный gas raymarch, затем terrain, не gas compute.
Числа314 исторические. Сначала conservative empty-space hierarchy/skipping
и корректная локальная композиция; потом reduced-resolution volumetrics
с depth-aware восстановлением. Световой кэш учитывает движущиеся препятствия,
свет/дым, камеру и damage; обновление через N кадров не точный кэш.

**Приёмка:** один replay, actual maximized pixels и контроль1600×900,
дистанции/покрытие дымом, overlapping regions, тонкие стены, быстрый поворот.
Ping-pong фон вне scissor, alpha/depth, ghosting и disocclusion проверяются.
Выигрывает полный GPU/frame, не лишь pass. Погрешность картинки явная;
SIM, игровая видимость и попадания прежние.

## OPT316-08. Владелец SIM и публикация

**08a:** использовать существующие CommandRuntime/receipts, HUD View,
geometry tables и terrain rows. Закончить inventory readers/writers:
moving/fragments/witness, terrain/streaming/shadows, газ, picking/inspection,
mission controller, save/reset. Перевести на immutable owned данные
ещё в синхронном адаптере и измерить полную цену экспорта.

Контракт: generation/tick/phase, geometry versions, позы двух тиков,
durable events/deltas, ack/release, bounded buffers. Latest-only допустим
для заменяемой позы, но не смерти/terrain edits/receipt. Медленный consumer
восстанавливает версию. Execution tick назначает owner: показанный tick+1
может устареть. Не сериализовать portable snapshot/clone мира каждый кадр.

**08b:** один постоянный owner thread, существующие pools; Main не берёт
mutex на весь tick. Явные pause/reset/load/save/shutdown/cancel/recovery,
overflow ingress, stale generation. Интерполяция использует simulation
timestamps и измеренный presentation lag. Новый контракт фиксируется
отдельно до включения владельца.

**Приёмка:** одинаковые принятые команды воспроизводятся headless,
согласованы поколения, bounded bytes/reclamation/backpressure; ordinary
frame tails,30 Гц,input latency/publication lag лучше/не хуже.
Отвязка кадра **не обещает ускорить сам SIM** или устранить GPU bottleneck.

## OPT316-09–10. Интеграция, память и масштаб

**09:** релевантные сцены после каждого среза, все26 после серии. Stock/saved
раздельно, actual maximized pixels,1×, камера, команды, fixed pools.
Native240 ordinary и отдельная диагностика;30 минут боя, reset/load cycles,
heap/VRAM plateau, редкий фриз со стеками и frame IDs. Pool4/8 сравнивать
в native: headless sweep не выбирает defaults.

**10:** [перенос экономики245](245-economy-port-plan.md) — проект механик,
не вторая performance-очередь. Continuations копки/placement, единицы
объёма, стройка, riders/fall damage и старые потребители `sim_core::World`
остаются обязательствами H3/H4. Это не удаление `voxel::World`, который
нужен `BlastRange`. Затем100/500/1000/5000 активных материальных юнитов
с боем, добычей, повреждениями и производством. Roster≤64 и старый5000
benchmark не квалифицируют этот масштаб.

## Единая приёмка изменения

1. Frozen baseline/source/data hashes, commands и конфигурация; один
   причинный production change. Тяжёлые проверки строго последовательно.
2. Точный SIM: independent oracle, gameplay counterexamples, per-tick
   counters/bytes, snapshot/replay, pools1/4/8. Full suite перед интеграцией
   серии, адресные проверки после малого шага.
3. Узкий stand → минимум две парные fixed-work серии, лучше три, ABBA.
   Раннее/позднее окна, p50/p95/p99/max, targeted phase **и root**,
   preparation/free/fallback. Числа разных backend не смешиваются.
4. Native ordinary и диагностика раздельно; темп/drops,frame tails,
   input/crater/tree/drain latency. Mean FPS и ratio0,99 не заменяют гейт.
5. Различие поведения точного кандидата — исправить/откатить. Регрессия
   времени/памяти за контрольный разброс — отклонить или доказать dispatch
   с полным fallback. Локальный gain называется локальным.
6. Отдельный отчёт результата; **статус обновляется только здесь**.
   Новое принятое решение — ADR и канон. Commit/push завершёнными порциями.

## Сведение прежних планов

| Старые ID/источники | Текущее место задачи |
|---|---|
| AUD314-01; ARCH315-03; EP-06; WBPERF-026 |03: capture, очередь, admission, progress |
| AUD314-02; ARCH315-01/04; EP-07 |01 выполнен;04/05 открыты |
| AUD314-03; ARCH315-02; EP-04/05 |02; collapse/claims также09 |
| AUD314-04 |04/05a: физика обломков |
| AUD314-05 |06: движение и навигация |
| AUD314-06 |05b: render CPU/mesh churn |
| AUD314-07/08; ARCH315-07 |07: GPU gas/terrain |
| AUD314-09 |08: публикация и владелец |
| AUD314-10/11 |00/09: attribution, память, native pools |
| AUD314-12 |10: масштаб текущего backend |
| ARCH315-05/06; EP-08/10 |05/06/07 |
| ARCH315-08; EP-02/03;240-B8 |08a/08b и native09 |
| EP-01; ARCH315-00/09 |00/09 |
| EP-09 | Допустимость целей исправлена; регрессии06/09, не новый rewrite AI |
| EP-11/12;240-H1–H5;245 |09/10; механики245 |
|240-C5; WBPERF-003/004/016 |06: latency групп |
| WBPERF-001/027 |00: трассы, provenance |
| WBPERF-002/006/007/008/009/020 |08: ingress/publication, UI, камера, terrain |
| WBPERF-005/011/017/028 |02/04/05: тела, admission, collapse |
| WBPERF-010/012/013/014/015/019 |02/05b/07: оружие, тепло, GPU, meshes |
| WBPERF-018/023/025 |09: память, lifecycle |
| WBPERF-021/022 | Регрессии движения/целей06/09 |
| WBPERF-024 |10: мир; render window не заменяет authoritative world |
|276/277/280/281/302/310/315| Обоснования/отвергнутые варианты сохранены; очереди заменены этой |

Вопросы продукта остаются в соответствующем каноне. Консолидация не
выбирает новый баланс/механики и не описывает будущие алгоритмы как внедрённые.
