# 214 — фазовые решения ИИ и воспроизведение без policy

Дата: 2026-10-08. Продолжение EP-02a после portable checkpoint 213.
Цель: воспроизводить бой по фактически вычисленным решениям обоих ИИ,
сохраняя общий движок, AfterMotion и прежнюю физическую нагрузку.
Это структурное изменение; ускорение обычного боя не заявляется.

## Изменение

`CommandRuntime::step_record_ai` возвращает один opaque `AiFrame` на AI
deadline, либо None в остальные тики. Кадр содержит Rules decisions enemy
и autonomous allied, вычисленные из одного observation после движения.
Enemy decisions также сохраняют потенциальные fallback, которые могут
потребоваться после отказа внешнего whole plan. Сам план и его receipt
живут в прежнем authoritative Inbox. Записанные правила выбора actor не
пересчитываются при replay, включая пустой список выбранных actors.

`step_replay_ai` никогда не вызывает policy. Он требует кадр ровно на
deadline, сверяет clock и hash владельца перед шагом, sides, slot order и
attack target bounds. Ошибка возвращается до drain player commands или
движения. Затем записанные decisions подаются в прежний общий handler:
исходный supplied plan, Outcome, fallback attempts, PlanReceipt и порядок
физики сохраняются. Internal assertion запрещает молчаливый возврат к
Rules при повторном потреблении replay input.

AiFrame имеет отдельный portable wire v1 `RTSAIF01`: little-endian поля,
строгие order tags/bool, checksum и aggregate caller ceiling решений до
выделения векторов. Внешний bin читает и пишет файлы; sim-core не делает
I/O. Checksum обнаруживает повреждение локального файла и не удостоверяет
происхождение policy. Replay не служит сетевым протоколом недоверенного ввода.

Hook принадлежит одному вызову, вынесен из клонируемого Controller и
удаляется до возврата. В owner нет скрытой будущей ленты, которую мог бы
потерять checkpoint. Physical format 53, его golden и runtime hash layout
не расширены. Нормальный `step` не пишет кадры, не сериализует и не считает
диагностический hash. При записи hash вычисляется только на AI deadline.

API удерживает только один фазовый набор с максимум одним решением на
actor. Caller должен резервировать ограниченный sink до вызова и сам
ограничивать хранение будущей ленты. None не скрывает пропуск: попытка
продолжения на deadline без кадра возвращает ошибку.

## Проверки и измерения

Headless `nav + sim-core --release`: **886 passed, 0 failed, 6 ignored**.
Семь новых проверок покрывают portable frames обоих ИИ, player queue и
retained plan receipts, фактический rejected turret Move + accepted fallback,
пустые решения реальных Base и terminal step, атомарный отказ при ошибке
deadline/hash/side/slot/target, все order tags, повреждение/усечение/лимиты
wire и фиксированный 41-byte little-endian conformance vector. Отдельный
контрпример подменяет recorded Attack на Hold: replay исполняет Hold,
хотя Rules для того же мира выбирает Attack; затем обычный step и restore
подтверждают отсутствие оставшегося transient replay mode.

`render --bin blast` check, build стендов, rustfmt и diff check прошли.
Architecture: **5 passed, 4 прежних failed**; имена и нормализованные
множества нормализованных диагностик совпадают с 213. Это известные нарушения
render writes, float, hash containers и clock; общего pass у harness нет.

Source passport: **374 inputs**. Изменены только `range_battle.rs`,
`range_command_runtime.rs`, `bench_command_runtime.rs`; добавлены
`range_ai_frame.rs` и его tests. Backup hash каждого изменённого исходника
равен паспортному 213; остальные 369 inputs совпадают. RON, dependencies,
баланс, физические caps, cadence и сценарии не менялись.

Compiler: rustc 1.98.1 (48a229cea, 2026-09-01), LLVM 22.1.8,
x86_64-pc-windows-msvc. Frozen binaries:

- Control 213 `bench_battle`: `3deb74e3c7558b8a663487a0b33ee8b4818584b68cb4588fadac2e2c918057c3`.
- Candidate `bench_battle`: `38634f45c7756dad403e4138f283729a9a7054a74c3811174ff1ae66a6a9a97a`.
- Candidate `bench_command_runtime`: `ccaccd511b2817af674e70fdd93035eff6c3aacc10e8bf3ded61d8133baaadd4`.

Smoke 15→16: один кадр 1035 bytes, original/in-process/fresh-process owner
hash `54f17223585ddfc3`, physical hash `098650392c175e47`; конечные portable
checkpoints оригинала и нового процесса побайтно равны.

Полная серия: **9 боёв**, каждый 60 units / 3600 ticks / 8 physics + 8 nav
workers, последовательно без параллельных builds/tests/CSV анализа. Stock
order: control 213 → candidate → candidate repeat → control repeat. Затем
owned-empty, plans-memory и три continuation probes с AI replay 3300→3600.
Default и filled дополнительно продолжены двумя независимыми процессами,
которые читают только сохранённый checkpoint и frame files.

| Stock run | Mean tick, ms | p99, ms | Mean AI scope, µs/tick |
|---|---:|---:|---:|
| Control | 13,198173 | 19,591 | 2,184167 |
| Candidate | 13,178679 | 19,581 | 2,009444 |
| Candidate repeat | 13,119969 | 19,556 | 2,166111 |
| Control repeat | 13,159548 | 19,651 | 2,010000 |

Средние пар: **13,178860→13,149324 ms (−0,224%)**; AI scope
**2,097083→2,087778 µs/tick (−0,009306)**. Прежний structural gate
≤+1% mean tick и ≤+5 µs AI пройден. Устойчивое ускорение и native FPS
не заявляются. Все **25** non-timing per-tick fields, damage geometry/ground
windows, конечные shots/damage/unit states совпадают с 213.
Stock hash **5dd0c782d2b2f557**, alive 26/21, bases 100/100, peak rounds 30.
У всех owner probes совпадают **6** non-timing fields и **12** физических
hash windows с соответствующим workload 213.

| AI replay 3300→3600 | Frames / bytes | Final owner hash | Physical hash | Новый процесс |
|---|---:|---|---|---|
| Default, обе стороны | 20 / 16280 | `97cb85edb31b44ed` | `5dd0c782d2b2f557` | Да |
| Whole enemy plans + autonomous allied | 20 / 16280 | `0a34e184769d1b2f` | `27735430f857d45d` | Нет |
| 1024 pending player requests + обе стороны | 20 / 16280 | `065314924271ae81` | `5dd0c782d2b2f557` | Да |

Все три in-process replicas имеют одинаковые с оригиналом final owner
hash и physical snapshot. Каждый replay использует decoded portable frames;
оба fresh-process final owner checkpoints **побайтно равны** оригиналам.
SHA256 final checkpoints: default
`2474a962e552891873c4958cf914ac143f22ca1c3e339f28f5ef4f803d2e9e28`,
filled `0ab1cba93ae72a882cb47f81b554a4a9df22ffccdeabd56f57bdd4ae755ea5b3`.
Каждый файл кадра содержит 814 bytes. Frame manifests/checksums записаны
в comparison/qualification JSON. Пропуск кадра не запускает Rules:
на AI deadline отсутствие файла даёт явный отказ step_replay_ai.

Whole-plan workload сохраняет **240 plans / 5982 decisions / 5737 attempts /
245 not selected / 0 fallbacks / 0 cancelled**, все исходные доставки и
результаты совпадают с 213. На clock 3300 сохранено 221 receipt, из них
один Delivered для ближайшей AI-фазы. В этом probe генератор будущих
внешних plans по-прежнему вызывает Rules снаружи owner и зеркалит envelopes;
внутренний AI replay policy не вызывает. Новым процессом future external
plan history ещё не воспроизводится. Реальный отказ и fallback покрыты
turret regression test; полный natural battle их не содержал.

Filled requests назначены на 3600..3615, поэтому к концу clock 3600 все
1024 остаются pending. In-process и fresh-process replay сохраняют их
полностью, вместе с IDs/sequences. Реальное исполнение player command
во время AI replay отдельно проверено в 31-tick differential test.
Checkpoint files default/plans/filled на clock 3300: 134186855 / 140888673 /
134248295 bytes, capacity=length. Codec encode/decode в одной пробе:
112,34/110,91; 114,81/115,56; 110,17/109,85 ms. Это вне step, без исходного
snapshot capture и полного physical restore. Plans/filled исходные files
имеют те же SHA, что 213; portable save/hash layout сохранены.

### Цена диагностической записи и повторная ревизия плана

Режим с полным pre-step owner hash намеренно **не включён по умолчанию**.
На 20 поздних AI deadlines средний recorded step составил **413,90 ms**
(default), **415,68 ms** (plans), **412,98 ms** (filled). Обычный owner на
тех же tick indices дал 16,92 ms, plans-memory — 15,29 ms. Это стоимость
всего recorded step с serialization/hash; отдельный hash scope не измерен.
Frame codec/file write и replica step в эти числа не входят. Среднее по
всем 3600 steps diagnostic probes — 15,47 / 14,70 / 15,41 ms; сравнивать их
с обычным боем как speedup или приемлемый production overhead нельзя.

Следствие: полная сериализация мира на каждом AI deadline непригодна для
рабочего authoritative journal. Этот API принят как проверяемый offline
phase replay primitive. Production journal должен иметь bounded sink и
дешёвую связь с stream generation/ordinal/checkpoint boundary; полный
physical hash остаётся отдельной диагностической проверкой, с её явной
ценой и частотой. При реализации следующего этапа нельзя просто включить
нынешнюю diagnostic запись в main/sim thread и объявить отделение владельца
завершённым. Это уточнение внесено в основной план и backlog.

Артефакты: `out/engine-fixes-20261008/ai-decision-replay/` — scripts, logs,
CSV, frame files, checkpoints, frozen sources/binaries и passports.

## Граница результата и дальнейший план

Выполнен общий primitive записи/replay внутренней AI-фазы. Он сохраняет
прежние игровые правила и не переносит их в сценарий. Полная история
admissions/results/acks ещё не существует. Для continuation с новым
внешним whole plan, player command или ack caller должен доставить тот же
вход отдельно и в том же порядке; pre-step hash обнаруживает несоответствие.

Следующий шаг EP-02a: единый bounded event journal для accepted/rejected
admissions, назначений, фактических outcomes, AI frames и ack с явным
порядком и backpressure до authoritative мутации. Нельзя скрывать drop,
дублировать receipts до бесконечности или сериализовать весь мир на каждый
тик или AI deadline. Для production записи сначала убрать из обязательного
пути полный world hash; существующий строгий offline replay guard сохранить
как диагностический режим с отдельной qualification. Затем перевод оставшихся writers/UI на protocol, EP-02b read model
с экспортным memory/copy/free gate, EP-03 отдельный owner и lifecycle.

Native FPS, поздние physics debt/soak и четыре прежних architecture failures
этим пакетом не закрываются. Поведение команд и сценариные данные не меняются.
