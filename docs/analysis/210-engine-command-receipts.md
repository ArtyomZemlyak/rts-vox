# Общий вход приказов и ограниченная запись их исполнения

Дата: 08.10.2026. Следующий узкий шаг EP-02a [плана](200-engine-performance-plan.md), после [общего обработчика 209](209-engine-shared-order-handler.md). Это структурная подготовка командного протокола. Авторитетный журнал replay и отдельный владелец симуляции ещё не реализованы.

## Проблема и проверяемая цель

Общий kernel из 209 возвращал только bool. Отказ валидации, неудачный поиск с его изменениями Motion, применённый приказ, повторное поддержание решения ИИ и deferred поиск формации нельзя было различить. Вход группового Move ещё отдельно выполнял queue/commit. Не было записи, позволяющей увидеть, какое решение внешнего плана действительно применилось и где потребовался fallback.

Цель — единый синхронный вход существующих боевых приказов с явным результатом, общая очередь formation и ограниченное наблюдение фактического применения. Фазы, scheduler и физическая работа сохраняются. До замера задана допустимая цена structural adapter: не более +1% среднего полного tick по двум controls/двум candidates и +5 мкс/tick в AI scope при выключенной записи. Capture проверяется отдельно по равенству состояния/работы, его цена не выдаётся за default performance. Новый gameplay SLO этим не вводится.

## Контракт исполнения

[commands](../../crates/sim-core/src/range_commands.rs) содержит типизированные `Order` и `FormationMove`. `execute_battle_command` — синхронный вход игрока, старые `battle_order`/`battle_group_move` совместимы через него. ИИ использует тот же validator/executor с прежним режимом Maintain, но остаётся после Motion перед `battle_orders`. Это не перенос ИИ во внешнюю фазу.

| Outcome | Прошла валидацию | Прежний accepted bool | Значение |
|---|---|---|---|
| Rejected | Нет | false | Фаза/сторона/актор/цель/содержимое отклонены |
| PathFailed | Да | false | Move или оба destination формации не выполнены; прежние эффекты неудачи Motion сохраняются |
| Unchanged | Да | true | Поддерживаемое решение ИИ уже действует/достигнуто; состояние не сбрасывается |
| Applied | Да | true | Немедленный приказ применён |
| Queued | Да | true | Destination формации принят, поиск идёт под прежним бюджетом Motion |

Queued не подтверждает достижение цели. PathFailed не обещает rollback: прежний immediate Move может очистить маршрут перед неудачным поиском. Формация сохраняет `queue_move(target) || queue_move(fallback)`, source order, дубликаты и частичное принятие. Ручной Move читает current rubble, ИИ — прежний фазовый rubble. Заряд, активный Spider pulse и Hold/reload прежние.

## Запись исполнения

`start_battle_command_capture(max_records)` включает observer явно; по умолчанию он отсутствует. Начало с нулевым пределом, невозможная allocation и повторный start отклоняются без изменения физики/существующей сессии. Запись содержит command/outcome, requested/assigned tick, phase, producer и его sequence. Player записывается BetweenTicks с текущим `BlastRange.tick`; ИИ — AfterMotion с scheduler next_tick как requested и фактическим `BlastRange.tick` как assigned. Этот clock увеличивается **в конце** step: первые внешние приказы и первое применение ИИ имеют stamp 0, но разные phase. После завершения этого step состояние имеет tick 1. Существующий clock и расписание сохранены.

Producer различает Player, EnemyRules, EnemyPlan, EnemyFallback и AlliedRules. Внешний выбранный приказ записывается отдельно от fallback, если применение отклонено. Повторный неизменный приказ и неудача пути тоже видны; запись только accepted bool потеряла бы важные эффекты/попытки.

Sequence отсчитывается с нуля для каждого producer **в данной capture session**. Он не является сохраняемым сетевым ID, не задаёт приоритет и не дедуплицирует ввод. Фактический порядок — append order единственного владельца. Capture не переупорядочивает команды и не вызывает policy повторно.

Память ограничена явно выбранным числом фиксированных Record, без allocation на каждую запись. При переполнении растёт `omitted`; последующая физика и подтверждения прежние. `complete()` означает отсутствие пропусков в наблюдаемом входе этой сессии, а не полноту всех writers игры. У отклонённой по фазе большой группы записывается только доступный prefix, оставшиеся элементы учитываются одним omit, без обхода всей группы. Для допустимой группы прежний обход запросов ещё сохраняется; общий bounded drain этим не введён.

`finish_battle_command_capture` возвращает записи и освобождает observer у runtime. Reset/новый Battle и restore начинают без него. Capture/sequence/omitted не читаются игровой логикой и не входят в hash/save. Это диагностическая запись исполнения, **не авторитетный command journal**: сохранённые команды не подменяют контроллер и не являются самостоятельным replay input.

## Проверка

[Новые tests](../../crates/sim-core/src/range_command_tests.rs) проверяют различия Rejected/PathFailed/Applied/Queued, fallback destination и stamps; реально отклонённый Move неподвижного врага из внешнего плана и применение правил; per-producer sequence и Unchanged. Tank/Spider выполняют настоящий 240-tick бой, ручные приказы и restore одного пути: запись включена только у другого пути, полные snapshots/hash/ledger сравниваются каждый tick, требуется фактическая стрельба и повреждение. Restore captured snapshot также исключает только observer. Проверка переполнения требует сохранения физического Move после исчерпания capacity и учитывает 100 000 отклонённых участников без незаметной потери записи.

Первый прогон выявил неправильное ожидание assigned_tick=1 в новом тесте: `BlastRange.tick` увеличивается в конце step, поэтому правильный stamp первого AfterMotion — 0. Исправлены только ожидание и описание clock; порядок фаз и production stamps не менялись. Отказ сохранён в `wrong-tick-fixture-tests.log` и отдельно воспроизведён в `fixture-failure.log`. Приёмка относится к повторному полному прогону исправленного дерева.

Headless [bench_battle](../../crates/sim-core/src/bin/bench_battle.rs) получил `--command-csv <path>` и необязательный `--command-limit <N>` (default 100 000 diagnostic records). CSV записывается после цикла, отдельно от tick CSV; summary явно содержит records/omitted/complete. Debug representation содержимого предназначено для анализа этого source/build, а не стабильного save/replay формата. Одновременно с group-only/restore-only ранним выходом exporter отклоняется. В обычном запуске observer и запись файла отсутствуют; gameplay-числа или квоты flags не меняют.

## Полный battle gate

Четыре последовательных world AI-vs-AI runs, 60 юнитов, 3600 ticks, physics/navigation workers=8/8, одинаковые damage trace/CSV: control 209 → candidate → candidate → control 209. Пятый отдельный run включает capture с пределом 100 000. Наши сборки/тесты и предварительный анализ CSV во время серии не выполнялись; comparison выполнен после всех пяти runs.

| Run | Средний tick, мс | p99, мс | Max, мс | Средний AI scope, мкс/tick |
|---|---:|---:|---:|---:|
| before | 13,181 | 19,521 | 33,724 | 2,217 |
| after | 13,120 | 19,441 | 33,689 | 2,056 |
| repeat | 13,167 | 19,532 | 33,662 | 2,117 |
| before-repeat | 13,208 | 19,511 | 32,895 | 2,272 |

Средние двух controls/двух candidates: полный tick **13,195→13,143 мс (−0,39%)**, AI scope **2,245→2,086 мкс/tick (−0,159 мкс)**. Среднее run p99 19,516→19,487 мс (−0,15%), среднее maxima 33,310→33,676 мс (+1,10%). Это средние индивидуальных run квантилей/максимумов, не объединённая выборка. Разброс candidate means 47,272 мкс сопоставим с изменением группового среднего 51,147 мкс; четыре runs без доверительного интервала не подтверждают устойчивого ускорения. Предварительный gate переходной цены соблюдён; пакет принимается по структурной цели, **без нового speedup/FPS заявления**. Rare stalls и хвосты не устранены.

На каждом tick совпали **25** non-timing CSV fields; физические damage windows, geometry/ground counters, shots/damage/phase/alive/bases прежние. Hash `5dd0c782d2b2f557`, alive `[26,21]`, bases `[100,100]`, peak rounds 30. Незавершённые физические очереди сохраняются, их прогресс этим пакетом не исправлен и не пропущен.

Capture run тоже совпал по всем этим физическим данным. Он записал **11 682** records: EnemyRules 5584, AlliedRules 6098; Applied 226, Unchanged 11 456; omitted=0, complete=true. Все phase=AfterMotion, requested/assigned clocks равны, per-producer sequences непрерывны с нуля. Внешний выбор/fallback проверен отдельным новым test, поскольку штатный AI-vs-AI run не содержит внешних планов. Средний tick capture run 13,199 мс: один диагностический run не изолирует цену observer, не сравнивается как default speedup и не квалифицирует native FPS.

Короткий отдельный CLI run 16 ticks с limit=2 экспортировал ровно 2 строки, omitted=114, complete=false. Exporter не скрывает усечение и не выдаёт неполную запись за полный поток команд.

## Квалификация дерева

Повторный полный nav/sim-core release/locked/offline test прошёл: **854 passed, 0 failed, 6 ignored** (111 nav, 743 sim-core включая bin/integration). Renderer blast check и форматирование пяти исходников прошли. Architecture снова даёт 5 passed/4 failed: прежние четыре отказа, без новых нарушений; весь workspace зелёным не объявляется.

Отдельный поздний replay **3300→3600 прошёл**, snapshot 134 186 763 bytes, layout 59 377 bytes, hash `5dd0c782d2b2f557`. Нормализованные architecture diagnostics равны принятому 209; новые файлы в отказах не фигурируют. Итоговый passport проверяет все 366 inputs и main/frozen SHA, tests/render, четыре default runs, captured workload/sequence/полноту, явный CLI overflow и replay. `git diff --check` прошёл.

Frozen control 209 SHA256 `4a0f22d89c3d8230e32b1d38a050b3fbd7691f59711b71b530e1a83201e4c0ac`; новый main/frozen SHA256 `22eef8138266354021cde46fe1cc4df16415e10a1634971ae0a2c5b30056d1e7`. **366 inputs**: изменены только прежние range_battle.rs, range_orders.rs и bin/bench_battle.rs; добавлены range_commands.rs/range_command_tests.rs, остальные 361 прежних inputs равны контролю. Все три backups проверены против исходных SHA. Toolchain Rust 1.98.1 (`48a229cea`, LLVM 22.1.8), x86_64-pc-windows-msvc, release, physics/nav=8/8.

Raw материалы — `out/engine-fixes-20261008/command-receipts/`: sources/backups, after-inputs.json, frozen binary, tests/build/render/architecture logs, пять CSV/log, commands.csv и commands-overflow.csv, comparison.json, capture-qualification.json, late replay и qualification.json. Каталог ignored; ключевые результаты записаны здесь, raw материалы другого checkout не обещаются.

## Оставшаяся граница EP-02a

Наблюдение не записывает delivery/whole-plan rejection, configure/controller changes, start/reset, aim/fire отдельной пушки, knobs/placement/loadout. Оно не проверяет внешний producer sequence/generation, не назначает future tick и не подтверждает доставку по межпоточному каналу. Эти contracts и записываемое авторитетное состояние ещё нужны для command replay без повторного запуска ИИ. Сохранение observer в snapshot вместо этого протокола не решает задачу.

Следующий шаг — внешний envelope и owner-side scheduling/ack на нынешних фазах, с записью принятого плана/fallback и lifecycle. Затем миграция остальных writers, полный read model EP-02b и export gate EP-03a. EP-03, физическая вместимость/прогресс EP-04b/c, historical work EP-07, native FPS и большой roster остаются открытыми.
