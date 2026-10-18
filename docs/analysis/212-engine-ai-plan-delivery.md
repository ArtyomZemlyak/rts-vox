# Доставка целого плана ИИ и подтверждение исполнения

> Исторический документ: цифры и статусы относятся к указанной здесь ревизии.
> Единственная текущая очередь — [план316](316-optimization-delivery-plan.md);
> свежая проверка и сведение источников — [аудит321](321-engine-performance-review.md).
> Новые статусы в этот документ не копируются.

Дата: 08.10.2026. Продолжение EP-02a после [211](211-engine-command-runtime.md). Внешний план передаётся тому же BlastRange, через прежний validator и ту же фазу AfterMotion. Сценарные данные, физические квоты и частота решений не меняются. Миграция UI, переносимый журнал и отдельный поток ещё открыты.

## Проблема и гипотеза

CommandRuntime принимал только приказы игрока, а mutable world accessor отсутствовал. Поэтому новый владелец не позволял доставить внешний enemy plan. Старый submit_enemy_ai_plan подтверждал только whole-plan валидацию; выбранное решение могло затем отклониться и вызвать fallback. Результат исполнения отсутствовал в восстанавливаемом Inbox. План на будущий decision_tick также сохранялся после завершения боя, хотя Controller::load отвергает pending plan вне Fighting.

Цель — атомарная доставка и удерживаемый результат целого плана, без переноса ИИ на начало tick и без дробления плана по player budget. До измерения задан structural gate обычного stock AI-vs-AI: не более +1% среднего полного tick двух controls/двух candidates и +5 мкс/tick в AI scope. Owner/planned runs проверяются отдельно по эквивалентности и фактической работе; их одиночные timings не объявляются speedup. Oracle: direct legacy delivery против owned delivery, независимый observer fallback, pending/completed checkpoint, terminal save, default workload/hash и полный бой с внешними планами.

## Реализованный контракт

PlanEnvelope содержит epoch/producer/sequence, decision_tick и slot-ordered Decisions всех живых врагов. ID(epoch, ordinal), watermarks и logical retention capacity общие с ручными приказами. Ошибки поколения, producer, sequence, capacity, schedule или содержания не изменяют мир, IDs и Inbox. World validator сохраняет проверки Fighting, точного next_tick, отсутствия второго pending plan, полного покрытия живых врагов, slot order, живой allied Attack-цели и Move bounds. Runtime сначала ограничивает длину roster, поэтому заведомо огромный input не порождает пропорциональных ему allocations.

Успешная доставка создаёт PlanReceipt(Delivered), сохраняя delivered_tick и исходные решения. План поступает в прежний Controller.pending сразу и ожидает свой decision_tick. Per-tick лимит player queue его не сдвигает и не разделяет. Текущее ограничение — один pending enemy plan на scheduler cycle; работа фазы по-прежнему ограничена существующим roster и cadence из data/battle-ai.ron.

Общий Rules::whole_plan дополняет вывод policy командой Hold для живых невыбранных actors в slot order наблюдения. Это адаптер движка для внешней доставки, а не правило конкретного сценария. Rules::decide и штатный внутренний цикл выбора actors не меняются. Произвольный неполный внешний план по-прежнему отвергается validator целиком.

В AfterMotion фиксируются фактический execution tick и source-ordered PlanAttempts: исходное Decision, Outcome и флаг fallback. При rejected/path-failed выбранном решении записывается также реально применённый rules fallback и его Outcome. Правила выбирают живых armed/mobile actors. Погибшие между доставкой и решением, а также живые неподвижные без оружия (например Base), не выбираются: их исходное решение остаётся в envelope, а selected attempt нет. Отсутствие attempt само по себе не доказывает смерть. Result Executed подтверждает потребление плана, а не успешное применение каждого решения или достижение Move-цели.

Результат удерживается до acknowledge_plan. Delivered нельзя удалить подтверждением; completed/cancelled можно. Старые IDs другого epoch и повторная sequence не исполняются снова. Capacity включает ручные requests/results и whole-plan receipts вместе; Full для нового ввода не удаляет принятый план.

При завершении боя до decision_tick pending plan очищается в общем kernel, а его owned receipt получает Cancelled с tick завершения. Это исправление terminal save распространяется и на старый прямой submit_enemy_ai_plan. Правила выбора победителя, движение/урон/материя не меняются. Hash терминального состояния с ранее висевшим pending plan закономерно меняется; такой старый snapshot не проходил существующий fighting guard loader. Default бой без внешнего pending plan сохраняет прежний hash.

## Память и checkpoint

Plan metadata хранится во втором preallocated Vec с capacity retained; shared logical limit учитывает обе очереди, а не два независимых лимита. Payload каждого retained plan ограничен размером неизменяемого roster. Владелец копирует decisions в компактный Vec: входной Vec с малой len и огромной caller-controlled capacity не сохраняется. Pending controller copy также компактна. До доставки резервируется buffer максимум 2*decisions PlanAttempts; каждый выбранный актор даёт не более selected+fallback. В фазе исполнения этот buffer не растёт allocations.

Transient PlanExecution находится в Battle вне Controller, поэтому перенос controller не клонирует новый result buffer. Hook отсутствует в обычном прямом runtime. Он не читается боевыми правилами и не входит в physical hash/save. После step владелец забирает его в authoritative PlanReceipt. Pending checkpoint хранит план и receipts в Inbox, а controller pending — в стандартном world snapshot; restore сверяет их и заранее восстанавливает bounded buffer. Completed/cancelled результаты восстанавливаются из Inbox.

Runtime hash дополнен всей metadata, исходными решениями и результатами plans. Его layout расширен; physical save/golden format сохранён. Checkpoint остаётся opaque типом программы в памяти. Это не переносимый file journal и не replay без policy: rules/fallback при checkpoint replay ещё вычисляются прежним контроллером. Сетевой transport/authentication и recovery потерянного admission reply не добавлены.

## Проверки

Headless release nav/sim-core: **869 passed, 0 failed, 6 ignored**. Восемь новых проверок покрывают atomic whole-plan admission, общие IDs/sequence/capacity с ручной очередью, compact retention при caller capacity 4096, selected rejection с независимой capture-проверкой реального fallback, pending/completed checkpoint, актор, погибший до решения, terminal cancellation и прямую/owned эквивалентность настоящих Tank/Spider боёв. Отдельно проверен общий Rules adapter: живой disabled actor получает Hold во входе, но не selected attempt в неизменной фазе.

Render `cargo check -p render --bin blast`, сборка двух release benchmarks, rustfmt и git diff --check прошли. Короткий owned whole-plan replay 15→16 совпадает по physical snapshot и runtime hash. Четыре прежних архитектурных отказа повторились при пяти пройденных checks: render writes, floating point, hash containers и clock reads. Имена и нормализованные диагностики совпадают с 211; это не прохождение всего architecture gate.

В начале проверки исправлен отсутствующий test import. Первый большой-world smoke выявил неполное покрытие Base выводом Rules::decide. Validator не ослаблялся: ошибка исправлена общим Rules::whole_plan с отдельным тестом. Неуспешные логи сохранены рядом с окончательными.

Паспорт относительно frozen 211 сверяет все **370 входов**: изменены пять существующих файлов (AI adapter, Battle hook, CommandRuntime, доступность общего test fixture и benchmark), добавлен один тестовый файл. Остальные входы неизменны. Замеры запускают frozen executables последовательно: четыре stock AI-vs-AI controls/candidates, empty owner и три external-plan боя (direct, owned, pending checkpoint replay). Физика и навигация — 8 workers, 60 units, 3600 ticks, общий штатный world preset.

Все восемь полных боёв прошли. Итоговый structural gate принят: два controls дают **13,193389 мс/tick**, два candidates — **13,196574 мс/tick (+0,024%)**; AI scope **2,110278→2,097917 мкс/tick (−0,012361 мкс)**. Это отсутствие измеренной существенной цены adapter в данной серии, не доказанное ускорение. Пики и native FPS не исправлены.

| Запуск | Средний tick, мс | p99, мс | Максимум, мс |
| --- | ---: | ---: | ---: |
| Stock control 211 | 13,228172 | 19,750 | 32,666 |
| Stock 212 | 13,168039 | 19,523 | 34,105 |
| Stock 212 повтор | 13,225108 | 19,630 | 34,546 |
| Stock control 211 повтор | 13,158607 | 19,747 | 33,449 |
| Empty owner 212 | 13,217184 | 19,7179 | 35,0609 |
| External plans direct | 12,450842 | 17,1970 | 43,9125 |
| External plans owned | 12,380495 | 17,0954 | 43,4837 |
| External plans owned + replay | 12,479782 | 17,3045 | 42,9803 |

Stock oracle: все 25 non-timing fields каждого из 3600 ticks, все физические damage/geometry/ground windows, итоговые shots/damage и состояния юнитов равны frozen 211. Physical hash **5dd0c782d2b2f557**, alive **26/21**, bases **100/100**, peak rounds **30**. Empty owner также сохраняет шесть workload fields, 12 window hashes и итоговую физику предыдущего owner.

External input oracle: три прогона имеют одинаковые **240 планов / 5982 решений**, включая delivery tick, decision tick, sequence и конкретные решения. План 0 доставлен на tick 0, дальнейшие — за один tick до фаз 15…3585. Для direct/owned/replay совпадают все шесть workload fields каждого тика и 12 physical window hashes. В каждом бою **3770 выстрелов**, alive **24/23**, bases **100/100**, peak rounds **30**, physical hash **27735430f857d45d**. Owned удерживает **240 Executed receipts / 5737 attempts**, **245 невыбранных решений**, **0 fallbacks / 0 cancelled**. Невыбранные включают Base; натуральный отказ/fallback в этой серии не встретился, его фактическая запись проверена отдельным тестом с Turret Move и независимым command capture.

План 3300 уже доставлен на tick 3299 и остаётся pending при checkpoint с clock 3300. Restore и продолжение до 3600 совпадают по physical snapshot, последующим входам и всем retained metadata/results: runtime hash **0a34e184769d1b2f**. Direct/owned snapshots побайтно равны: **152136090 bytes**, SHA-256 **2439b71fa4f09817875a8484dce051cfdbbe7ede08259ded5bef8430a1d1f133**.

Внешний planner получает observation до прежней AfterMotion-фазы, поэтому его выбранные входы и итоговый мир отличаются от stock policy. Это существующая семантика внешнего плана; direct и owned сравниваются на одинаковых входах. Средние external/stock нельзя сравнивать как speedup. Owner benchmark не имеет stock phase observer; policy/observation вычисляются вне timed step, delivery измеряется отдельно (direct **186700 ns**, owned **770800 ns** суммарно на 240 plans). Полный checkpoint/snapshot также не входит в step timing; его большой размер не является решением дешёвой публикации для рендера. Replay ещё вычисляет rules/fallback, это не replay без policy.

Паспорт binaries: stock 212 SHA-256 **029315a46ebf4b54fa261158a0375305ca2db8bb11e39405ccd6c7497ac9d667**, owner **78d2756d374098f477580c8e61fc13d73c20e63e2128324018f98d55ac8dbe93**. Frozen stock control 211 — **9ebfedfb332b38214d12ed80303e0b7c5905595ee83af137bfbfc7ebc0af2afa**. Compiler: rustc **1.98.1 (48a229cea 2026-09-01)**, LLVM **22.1.8**, x86_64-pc-windows-msvc. Raw artifacts: `out/engine-fixes-20261008/ai-plan-delivery`; passport — `after-inputs.json`, измерения — `*.csv`/`*.log`, итоговые gates — `comparison.json`/`qualification.json`.

## Следующий шаг

EP-02a остаётся частичным: versioned portable journal accepted inputs/results с явным replay без повторного policy, остальные writers (в том числе другие контроллеры сторон, standalone gun, knobs и lifecycle) и подключение UI. Далее модель чтения/export EP-02b/03a и threaded owner/lifecycle EP-03b/c. Цели physics capacity/progress и native FPS остаются открытыми; надёжная доставка планов не устраняет существующий backlog разрушения. В конце external боя felling pending **3543054**, collapse queued **1599420**, fragments **42275**: очереди не дренированы и этот пакет их не сокращает.
