# Ограниченная доставка внешних боевых приказов

Дата: 08.10.2026. Продолжение EP-02a после [210](210-engine-command-receipts.md). Это синхронный adapter над существующим BlastRange, с тем же kernel движения, оружия, разрушения и ИИ. Игровой интерфейс пока использует прежний синхронный вход; этап целиком не закрыт.

## Проблема и гипотеза

Outcome и диагностический capture не обеспечивали доставку: повтор запроса мог повторно очистить маршрут, массовый ввод не имел общего drain limit, а медленный потребитель мог потерять подтверждение. Нужен вход, который отдельно подтверждает назначение и фактическое исполнение, удерживает принятый запрос и не меняет фазу ИИ.

До замера задан gate переходной цены: средний полный tick двух empty-owner runs не более +1% относительно двух direct runs на том же исполняемом файле. Это структурная цель, не обещание ускорения. Отдельный run с 1024 будущими приказами проверяет цену заполненной очереди; он не заменяет empty-owner gate. Oracle: реальные Tank/Spider бои против прямого исполнения, физические snapshots/hash, явная активность оружия и повреждений, восстановление pending work; затем полный world AI-vs-AI 60 units/3600 ticks/8+8 workers. Менять сценарную физику/частоту ИИ/квоты разрушения запрещено.

## Реализованный контракт

[CommandRuntime](../../crates/sim-core/src/range_command_runtime.rs) получает уже начатый бой, nonzero epoch и явные Limits(retained, producers, per_tick). Мир принадлежит ему; наружу доступен только &BlastRange. Он не создаёт второй kernel и не запускает поток. Setup/placement/configuration выполняются до передачи мира владельцу. Новый/reset бой требует нового epoch от вызывающего кода. Per-slot generations, миграция UI/lifecycle и остальных writers остаются открытыми.

Envelope содержит epoch, dense producer ID, монотонный sequence с нуля, requested_tick и прежний Command. Приём отклоняет чужой epoch, producer вне диапазона, пропуски/повторы sequence, прошлый tick, исчерпанную capacity и overflow. Отказ не меняет watermarks, IDs, очередь или мир. Producer IDs служат дедупликации, не являются сетевой аутентификацией; все команды имеют полномочия прежнего player-входа. Врагами по-прежнему управляет существующий контроллер.

Назначается первый свободный tick начиная с requested_tick; per_tick ограничивает число внешних вызовов executor, а не физические фазы. Уже выданный assigned_tick никогда не сдвигается. Записи сортируются по assigned_tick/acceptance ID: будущая команда не блокирует более раннюю, одинаковый tick исполняется в порядке приёма владельцем. Нельзя обещать независимость от сетевого arrival order. Один bounded проход считает reservations, вставка/ack/drain ограничены retained; квадратичного повторного поиска заполненных тиков нет.

Receipt(None) подтверждает расписание. CommandId содержит epoch и ordinal: старый ack другого боя не может удалить новый результат с тем же ordinal. На старте соответствующего step, до всей физической работы, общий execute_battle_command выдаёт Some(Outcome); ИИ остаётся AfterMotion. Живой актор/цель и gameplay-содержимое проверяются при исполнении. Rejected/PathFailed также остаются подтверждаемыми результатами. Queued подтверждает destination Motion, а не достижение цели.

Capacity включает pending и исполненные, но ещё не подтверждённые записи. Поэтому принятый запрос заранее резервирует место для результата. Медленный consumer вызывает явный Full для новых запросов; симуляция и принятые запросы продолжаются. acknowledge удаляет только исполненный receipt, сохраняя producer watermark. Повтор после ack всё равно отклонён и не исполняется. После Full можно повторить ожидаемый sequence с допустимым requested_tick, когда место освободилось. Это локальный синхронный API: сетевой transport, аутентификация и восстановление потерянного admission reply не реализованы. Все allocations очереди выполнены до приёма; restore восстанавливает configured spare capacity.

## Состояние и восстановление

Opaque Checkpoint в памяти содержит стандартный physical snapshot и полный Inbox: epoch/limits, watermarks, next ID, assigned ticks, команды и результаты. Restore поднимает их совместно. Runtime state_hash включает physical hash и всё состояние Inbox, включая будущие команды/удерживаемые результаты. Только world().snapshot()/state_hash() недостаточно для восстановления командного runtime.

Checkpoint — тип текущей программы, не переносимый двоичный журнал. Existing BlastRange save/golden format не меняется. Durable encoding/versioning, запись принятых whole AI plans/delivery/fallback и replay без повторного запуска policy ещё требуются. Диагностический capture 210 не превращён в авторитетный input. Adapter не включён в UI по умолчанию и не объявлен завершением EP-03b/c.

## Проверки и измерения

Семь [новых tests](../../crates/sim-core/src/range_command_runtime_tests.rs) проверяют отказ без мутации, Full без продвижения sequence, retention результата до ack, повтор после ack, назначение под budget, future head blocking, source-order ties, Rejected при исполнении и защиту от старого ack другого epoch. Различающиеся pending-команды меняют runtime hash при одинаковой физике. Реальные Tank/Spider бои на 240 ticks сравнивают полный physical snapshot/hash после каждого step, требуют выстрелы и повреждения, восстанавливают два pending-приказа вместе с двумя completed receipts и продолжают одинаковый replay. Restore также проверен последующим приёмом команды и отказом повторной sequence.

Первый targeted прогон из шести tests и полный прогон из 860 прошли. При повторной ревизии выявлен пропуск поколения в ID acknowledgement: одного ordinal недостаточно после reset. Исправлен CommandId(epoch, ordinal), добавлен контрпример. Окончательное дерево пересобрано и прошло **861 nav/sim-core tests, 0 failed, 6 ignored**: nav 111, sim-core library 703, bench_battle 3 и integration 44. Приёмка относится к последнему прогону с этой защитой. Release build, renderer blast check, rustfmt и git diff --check прошли. Architecture: 5 passed/4 failed; названия и нормализованные diagnostics точно совпадают с 210. Это прежние the_render_layer_never_writes_into_the_simulation, the_simulation_has_no_floating_point, the_simulation_has_no_hash_containers, the_simulation_never_reads_the_clock; весь workspace зелёным не объявляется.

Артефакты находятся в out/engine-fixes-20261008/command-runtime. Паспорт проверяет **369** source/data/manifest inputs; изменены только range_commands.rs и range_command_tests.rs относительно 210, добавлены runtime, его tests и отдельный bench. Основной frozen executable SHA-256: `9ebfedfb332b38214d12ed80303e0b7c5905595ee83af137bfbfc7ebc0af2afa`; bench_command_runtime: `843ccfd796cefb1a2962257dac92628181135aca6f0a57c657dcda34ab9a6af3`. Контрольные inputs и frozen bench 210 с SHA `22eef8138266354021cde46fe1cc4df16415e10a1634971ae0a2c5b30056d1e7` сохранены. Rust 1.98.1/LLVM 22.1.8, Windows x86_64 MSVC, release/locked/offline, physics/navigation 8/8; dependencies и gameplay data не менялись. qualification.json повторно сверяет текущие input/binary hashes и все приёмки.

### Полный бой и цена adapter

[bench_command_runtime](../../crates/sim-core/src/bin/bench_command_runtime.rs) использует обычные range-km/battle-world данные и неизменный BlastRange. Он измеряет plain step без phase observers; результаты нельзя напрямую сравнивать по времени с прежним наблюдаемым bench_battle. Для нового gate direct/owned исполняются на одном frozen executable, в порядке direct → owned → owned-repeat → direct-repeat. CSV и snapshots пишутся после цикла; physical hashes каждые 300 ticks считаются вне измеряемого step одинаково у обоих путей. Во время серий наши builds/tests и анализ CSV не выполнялись.

| Run | Средний tick, мс | p99, мс | Max, мс |
|---|---:|---:|---:|
| direct | 13,899 | 23,972 | 214,949 |
| owned | 13,114 | 19,619 | 32,544 |
| owned-repeat | 13,081 | 19,657 | 33,320 |
| direct-repeat | 13,227 | 19,694 | 34,395 |
| direct-confirm | 13,205 | 19,863 | 33,589 |
| owned-confirm | 13,107 | 19,511 | 33,483 |
| owned-confirm-repeat | 13,175 | 20,077 | 34,813 |
| direct-confirm-repeat | 13,223 | 20,241 | 35,021 |
| filled, 1024 pending | 13,196 | 19,661 | 33,002 |

Первая серия дала групповой mean 13,563→13,098 мс (−3,43%), но control имеет max 214,949 мс и повышенный p99. Причина пика не установлена; этот run сохранён и не исключён из первого отчёта. Повторены ещё четыре полных боя на неизменной сборке. Confirmation means: **13,214→13,141 мс (−0,56%)**, в заранее заданном пределе +1%. Разброс owned means 68,290 мкс близок к разнице групповых means 73,419 мкс; четыре confirmation runs не доказывают устойчивого ускорения. Это приёмка переходной цены синхронного adapter, без speedup/FPS заявления и без квалификации редких stalls. Confirm maxima достигают 35,021 мс; весь tick tail не исправлен.

Filled — отдельный одиночный diagnostic run с 1024 будущими запросами. Они назначены на ticks 3600..3615, после измеряемых steps 0..3599, остаются pending и не подавляют ИИ/физику. Его время 13,196 мс не является отдельным доказательством цены одной команды или drain большого batch. Массовый admission/фактическое исполнение и slow-consumer retention проверены targeted tests; масштабный command-to-visible latency пока не квалифицирован.

Всего выполнено **11 полных 3600-tick боёв**: один stock observed, восемь paired direct/owned, один filled и один checkpoint replay. Stock run совпал с 210 по всем **25 non-timing per-tick fields** и всем damage windows, geometry/ground counters. Во всех десяти plain runs совпали шесть per-tick fields (tick, rounds, fragments, felling, collapse_queued, fired) и **12 physical window hashes**. Итоговый physical hash `5dd0c782d2b2f557`, alive `[26,21]`, bases `[100,100]`, peak rounds 30. Final snapshots direct/owned при tick 3600 равны побайтно: 145 420 428 bytes, SHA `92cb375039057a6d6e905047dda43b1f22ec79e5126bdae00f6104285636c6a4`. Это размер tick 3600, не размер прежнего checkpoint на tick 3300.

Отдельный replay берёт runtime checkpoint на tick 3300 с **1024 pending requests** и выполняет оба мира до 3600. Совпали physical snapshots и полный owner hash `04f826777f639861`, retained=1024. Запросы ещё не due: проверено восстановление их расписания/содержимого/watermarks, а не исполнение всех 1024 в этом окне. Этот replay исключён из performance gate. Оружие, накопленные фрагменты, валка и обрушение продолжают обычную работу; backlog физики этим пакетом не устранён.

## Следующие шаги

EP-02a: переносимый versioned journal и admission/исполнение whole-plan ИИ с записью fallback, остальные writers, подсоединение UI, полноценные generations и lifecycle. Далее EP-02b/03a: модель чтения и qualification экспорта позднего мира; после неё отдельный поток владельца EP-03b и полная приёмка reset/save/pause/shutdown EP-03c. Physics capacity/progress EP-04b/c и native frame/FPS остаются самостоятельными задачами. Переход к bounded command transport сам по себе не уменьшает стоимость разрушения.
