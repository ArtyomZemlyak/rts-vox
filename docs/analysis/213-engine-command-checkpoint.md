# Переносимый checkpoint общего владельца команд

Дата: 08.10.2026. Продолжение EP-02a после [212](212-engine-ai-plan-delivery.md), ADR-619. Это сохраняемый checkpoint мира и authoritative Inbox, необходимая часть будущего журнала. Полный журнал событий и replay без policy этим пакетом не заменяются.

## Проблема и выбранная граница

CommandRuntime сохранял физический мир, ручную очередь и whole-plan receipts только в opaque объекте памяти. Запись world().snapshot() теряла player reservations, results, IDs/watermarks и owned plan receipts; восстановление в новом процессе было недоступно. Новый формат объединяет эти данные, не добавляя файлы/I/O в sim-core и не меняя существующие physical save/golden.

Точка сохранения — между step, после передачи фазового plan result в Inbox. Checkpoint::to_bytes и from_bytes работают с байтами; файловое чтение/запись принадлежит вызывающему слою. Runtime::restore восстанавливает тот же общий BlastRange, проверки очереди и bounded pending-plan buffer. Сценарий задаёт обычный preset и подключает владельца, без отдельной реализации сохранения команд.

До измерения задан structural gate штатного AI-vs-AI: два controls/два candidates, не более +1% mean tick и +5 мкс/tick AI scope. Owner/codec timings диагностические, не speedup. Проверка эквивалентности — frozen 212, одинаковые per-tick workload/damage windows, memory/portable replay 3300→3600 и независимый новый процесс с файлом checkpoint. Никаких сборок/тестов/CSV анализа параллельно серии боёв.

## Формат и проверка

Wire v1: magic RTSCMD01 (8 bytes), u32 version, u64 world length, исходные bytes physical snapshot, Inbox, u64 FNV-1a checksum предшествующего payload. Числа little-endian; размеры/index записаны u64 и читаются с проверкой usize::try_from, ticks — u32, координаты — i32, enum/bool tags — u8. Неизвестные tags/version, неканонические bool, повреждение, обрезка и trailing bytes отвергаются.

Inbox включает epoch, Limits, next_id, producer watermarks, sorted player receipts/envelopes с requested/assigned tick и optional Outcome, ordered plan receipts/envelopes с delivered/decision tick и Delivered/Executed/Cancelled, selected/fallback Decisions/Outcomes. Уже acknowledged результаты отсутствуют, но watermarks и next_id сохраняются. Поэтому это снимок текущего владельца, не непрерывная история принятых команд.

DecodeLimits задаётся вызывающим слоем: предел world bytes, максимальные runtime retained/producers/per_tick и decisions одного плана. Код проверяет lengths против этих пределов и оставшихся bytes до резервирования payload Vec. Суммарная logical retention ручных и AI receipts остаётся общей; максимум attempts — два на исходное решение. Vec входных bytes не удерживается: decoder строит compact собственные копии.

Структурные guards проверяют поколения/producer/sequence, уникальность IDs, соответствие sum watermarks и next_id, сохранённый порядок IDs/sequence, сортировку назначенных player ticks, лимит reservations на tick и максимум один последний Delivered plan. После загрузки physical world restore проверяет прошлое/будущее ручных результатов, план и scheduler/pending controller, source actors/targets, decision order, execution/cancellation ticks и форму selected→fallback attempts. Эти проверки не требуют заново запускать ИИ.

Pending player input не подвергается новой gameplay admission: невалидный actor/target/Move остаётся в очереди до прежнего executor и получает прежний Rejected/PathFailed. Завершённый plan может содержать ныне мёртвого актора/цель; alive-at-admission нельзя восстановить из текущего мира. Fallback Move также может сам не пройти путь/границы: хранится реально выполненная попытка, а не заново валидированный план. Checksum обнаруживает повреждение, не аутентифицирует источник; embedded physical snapshot использует существующий loader доверенных локальных сохранений. Этот пакет не делает его безопасным сетевым parser.

Runtime hash использует тот же общий Inbox writer с прежним порядком bytes и tags; hash layout 212 не меняется. Wire header/checksum не добавляются в runtime hash. Нормальная фаза step не вызывает codec и не получает нового обхода очереди; validation выполняется при restore.

## Память и стоимость

Первый encoder мог заполнить Vec world bytes и удвоить его capacity при добавлении первого поля metadata. Исправлено до финальных замеров: metadata сначала сериализуется отдельно, большой output резервируется на точный конечный размер. World snapshot и decoded copy остаются большими полными буферами; это редкая операция сохранения, не дешёвая публикация состояния для рендера. Приёмка размера/стоимости read model остаётся EP-02b/03a.

## Проверки

Headless release nav/sim-core: **879 passed, 0 failed, 6 ignored**. Все десять новых проверок прошли: fixed wire v1 conformance vector; mixed pending player/AI + scheduler, completed results, ack и watermarks; фактический rejected selected + fallback; cancelled terminal plan; все player Command/Order варианты с невалидными gameplay inputs; реальные Queued/PathFailed results; повреждение, truncation, versions/tags/bool/trailing bytes; caller bounds и огромные длины; IDs/order/watermarks/capacity/timeline; pending mismatch и невозможная форма selected/fallback.

Render check blast, сборка двух release benchmarks, rustfmt и git diff --check прошли. Архитектурная проверка повторила **5 pass / 4 прежних fail** (render writes, float, hash containers, clock); имена и нормализованные diagnostics равны 212. Полный architecture gate ещё не пройден.

Smoke: сохранён pending plan при clock 15, **2 receipts**, **2552528 bytes**, capacity точно равна len. Encode **2,0282 мс**, bounded decode **2,0129 мс** — одиночные diagnostic samples, не постоянная стоимость. Отдельный процесс загрузил файл и переписал его побайтно без изменений: physical hash **2e54b47b60928920**, owner hash **22efabcb1cd28169**. Продолжение 15→16 совпало с original runtime: owner hash **1f7d50d344a4da42**. Codec/restore не включаются в timed tick.

Паспорт относительно frozen 212 проверяет **372 входа**: изменены только CommandRuntime и benchmark, добавлены codec и его тесты. Игровые данные, physical kernel и фазы прежние. Серия: четыре stock controls/candidates, empty owner, whole-plan memory replay, whole-plan portable replay, portable replay с 1024 pending player requests. Все бои — 60 units/3600 ticks, physics/navigation workers 8, штатный world preset. После timed серии два файла проверяются в независимых новых процессах.

Все восемь полных боёв и две независимые file roundtrips прошли. Structural gate принят: mean двух stock controls **13,183753 мс/tick**, двух candidates **13,195718 мс/tick (+0,091%)**; AI scope **2,047917→2,070833 мкс/tick (+0,022917 мкс)**. Разница не доказывает ни устойчивого ускорения, ни причинной регрессии. Пики и native FPS не квалифицированы.

| Запуск | Средний tick, мс | p99, мс | Максимум, мс |
| --- | ---: | ---: | ---: |
| Stock control 212 | 13,177913 | 19,499 | 33,392 |
| Stock 213 | 13,200205 | 19,639 | 34,901 |
| Stock 213 повтор | 13,191231 | 19,707 | 37,519 |
| Stock control 212 повтор | 13,189594 | 19,602 | 37,059 |
| Empty owner | 13,228715 | 19,7051 | 33,1141 |
| External plans + memory replay | 12,499170 | 17,4951 | 43,2148 |
| External plans + portable replay | 12,552065 | 17,5684 | 43,3284 |
| 1024 pending requests + portable replay | 13,277169 | 19,9269 | 34,2218 |

Stock: все **25 non-timing fields каждого тика**, damage/geometry/ground windows, итоговые shots/damage и состояния юнитов равны frozen 212. Hash **5dd0c782d2b2f557**, alive **26/21**, bases **100/100**, peak rounds **30**. Empty/filled owner сохраняют шесть workload fields и 12 physical window hashes прежнего обычного боя.

Memory/portable external-plan runs сохраняют шесть workload fields/12 hashes, те же **240 планов / 5982 решений**, конкретные delivery/decision ticks и decisions, **240 results / 5737 attempts / 245 not selected / 0 fallbacks / 0 cancelled** из 212. Final physical hash **27735430f857d45d**, owner hash **0a34e184769d1b2f**. Доставка policy до AfterMotion остаётся отдельным workload относительно stock; меньшие средние external runs не объявляются speedup.

При clock 3300 whole-plan checkpoint содержит **221 receipt**: 220 выполненных и ожидающий план 3300, доставленный на tick 3299. Второй checkpoint содержит **1024 pending player requests**, назначенных на ticks 3600…3615. Оба продолжения 3300→3600 совпадают с оригиналом по physical snapshot и runtime hash. Новые процессы загружают файлы без исходного объекта Checkpoint и переписывают их побайтно без изменений.

| Checkpoint 3300 | World bytes | Wire bytes = capacity | Добавлено к world, bytes | Encode, мс | Decode, мс |
| --- | ---: | ---: | ---: | ---: | ---: |
| Whole-plan owner | 140680236 | 140888673 | 208437 | 117,1189 | 114,8498 |
| 1024 pending player | 134186763 | 134248295 | 61532 | 110,9180 | 109,7656 |

Это одиночные codec samples с checksum и копированием; full physical snapshot capture и BlastRange::restore в них не входят. Codec, snapshot, hash diagnostics, file I/O и replay replica выполняются вне timed original step. Peak native frame/load/save latency и high-water памяти не измерены. По размеру основной расход остаётся physical snapshot, поэтому этот формат нельзя использовать для ежетикового render export.

Whole-plan file SHA-256 **b52e1bd53ca412aee37231cdb0fa044dd8f755bea83e6776f49fab9ed8f4301d**, owner hash при сохранении **90f71a33f7b75cf4**, physical hash **52160782cf0a85aa**. Filled file SHA-256 **36f4e8fc8be10944283f0147214366e16440af491779237333925def865226ed**, owner hash **32a5a10ca10a03a4**, physical hash **60bc6fb338a5ed21**. File hashes независимых roundtrip совпадают.

Frozen stock 213 SHA-256 **3deb74e3c7558b8a663487a0b33ee8b4818584b68cb4588fadac2e2c918057c3**, owner **d5b63bc0413dae840ac573575ed55ee67e9dd7001fd11e7158ab13d255dcfa4f**; control stock 212 **029315a46ebf4b54fa261158a0375305ca2db8bb11e39405ccd6c7497ac9d667**. Compiler **rustc 1.98.1 (48a229cea 2026-09-01), LLVM 22.1.8, x86_64-pc-windows-msvc**. Raw artifacts: `out/engine-fixes-20261008/command-checkpoint`; `after-inputs.json`, `*.csv`/`*.log`, `*.checkpoint`, `comparison.json`, `qualification.json`.

## Следующий шаг

EP-02a: versioned bounded журнал принятых inputs и фактических AI decisions/fallbacks с фазами, полная история ack/delivery и replay без повторного policy. В нынешнем checkpoint восстанавливаются ожидающий план, retained results и счётчики, но правила продолжают вычисляться прежним controller. Остальные writers/UI, read model/export EP-02b/03a и threaded lifecycle EP-03b/c открыты. Physics capacity/progress и native FPS не закрываются сохранением очереди.
