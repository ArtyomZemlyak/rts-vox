# 322 — Компактные диапазоны опорной карты завалов

18.10.2026. Before: `main 3c2e8f1`, runtime как в321. Изменение общего
`Bursts::rubble_with_scratch`, ADR-747, подготовительный срез OPT316-04.
Межтиковые bounds/membership04a и зависимости опор04b остаются открытыми.

## Найденная причина и выбор изменения

Свежий before World6000 с `--profile-rubble`,physics4/nav8/Rayon4:
в позднем окне2401–6000 вся rubble7,278 мс/тик. Её последовательные
parent spans: classify1,315, support build1,533, query1,027,
columns3,033 мс. В columns уже входят gather1,038 и merge1,258 мс:
их нельзя складывать с columns. Самая большая из этих стадий —
построение итоговых XZ колонок после проверки опор.

Первая проба сохраняла локальные bounds обычных фрагментов между тиками
по полному ключу shape/mm/side/basis. Она **отвергнута и удалена**:
classify1,315→1,320 мс, дополнительный sidecar≈9 МБ на69 тысяч записей.
Весь mean18,259→18,228 мс не доказывает выигрыш этой стадии; небольшая
разница пришлась на другие работы. Cache invalidation была точной,
но добавление кэша само по себе не оказалось полезной оптимизацией.

Принято другое изменение по тому же профилю: не разворачивать каждый
footprint в список повторных клеток до вычисления максимумов колонок.
Это уменьшает подготовку карты опор без нового физического расписания.

## Как устроен новый путь

1. Classify, контакт с текущим грунтом, поддержка другим фрагментом и
   исключение hull riders выполняются прежним алгоритмом на текущем входе.
2. Для каждого принятого floor fragment готовится XZ прямоугольник,
   прежний rounded/clamped top и маска корпусов, которые могут закрывать
   его отдельные клетки. Near и исключение корпуса по центру прежние.
3. По counts/prefix scatter диапазон распределяется по X-строкам как
   Z-range/top/hull-mask. Каждая читаемая запись перезаписана текущим входом.
4. Независимые полные строки сразу сводят диапазоны в точный max по Z;
   потенциально влияющая клетка сохраняет `Hull::covers`. Если имеющаяся
   высота уже не меньше нового вклада, проверять его укрытие не требуется.
5. Два уровня occupancy summaries выдают только занятые колонки в X/Z
   порядке и обнуляют затронутые временные entries. Top+1 сохраняет0
   и65535. Indexed parallel work не выдаёт результат по порядку завершения.

Прежний gather создавал по записи на каждую колонку каждого фрагмента,
потом ещё раз распределял их по X и схлопывал повторные высоты. Новый
не материализует этот полный список. Память диапазонов/строк всё равно
нужна; снижение RSS/VRAM по одному уменьшению списка не заявляется.

Dispatch: не менее1024 принятых floor fragments,≤64 hulls и достаточно
плотный16-bit XZ domain. Меньшие/разреженные/широкие входы и>64 hulls
сохраняют полный прежний builder.64 — ширина маски, не ограничение сцены.
Sparse fallback включает стоимость предварительной проверки плотности.

Сохранены physical phases, опоры, формы, same-tick nav/collision, исходные
ID/порядок, logical quotas, wakes/landing, энергия/ledger и snapshot/hash.
Сценарных условий и изменений игрового баланса нет. Scratch не является
кэшем ответов опор: world/hulls/fragments читаются заново при каждом входе.

## Первый сравнительный профиль

Frozen before и опытный spans release, по6000 одинаковых тиков,
physics4/nav8/Rayon4; в обоих включён только одинаковый parent observer.
Это первичная диагностическая пара, ещё не квалификация обычного FPS.

| Scope /окно | Before мс/тик | Spans мс/тик |
|---|---:|---:|
| Весь root1–6000 |18,259|17,344|
| Вся rubble1–6000 |5,336|4,594|
| Весь root2401–6000 |21,031|19,721|
| Вся rubble2401–6000 |7,278|6,230|
| Columns2401–6000 |3,033|2,118|
| Gather внутри columns2401–6000 |1,038|0,479|
| Merge внутри columns2401–6000 |1,258|0,922|

По всем6000 тикам совпали36 нетайминговых CSV-полей; итог
`21bef878463f364c`. Columns примерно−30%, rubble поздно−14,4%, весь mean
примерно−5%. Не вся разница root причинно относится к columns: в одной
паре меняются и неизменённые стадии, поэтому нужны повторные контроли.
Большие залповые capture/frontier/apply этим изменением не исправлены.

## Повторная квалификация CPU

Выбранные контрольные runs в порядке before-a,after-a,after-b,before-b,
каждый6000 тиков с теми же pools и parent observer. After-a — первичный
spans binary; after-b — финальная release сборка с восстановленным leaf
маркером gather, который не входит в выбранные parent scopes. Между первой
before и spans проверялся отдельно отвергнутый geometry cache.
Это две парные проверки целевой нагрузки, не случайная выборка всех машин.

| Run | Root mean мс | Root p99 мс | Root max мс | Rubble mean мс | Late rubble мс | Late columns мс |
|---|---:|---:|---:|---:|---:|---:|
| before-a |18,259|45,317|106,451|5,336|7,278|3,033|
| after-a |17,344|43,780|105,369|4,594|6,230|2,118|
| after-b |17,192|44,129|105,944|4,532|6,144|2,031|
| before-b |17,993|43,616|105,742|5,202|7,096|2,904|

В двух парах: columns поздно−30,0…−30,2%; вся rubble поздно−13,4…−14,4%;
весь mean−4,5…−5,0%, late root−5,6…−6,2%. По36 нетайминговым полям
каждый тик совпал во всех runs; hash6000 `21bef878463f364c`.
P99 root во второй паре вырос43,616→44,129 мс, поэтому устойчивое
ускорение хвостов не заявляется. Max остаётся≈106 мс; every-tick33 мс
не пройден. Включены все расходы подготовки ranges/scatter/free и fallback.

Финальные SHA-256 (source/data fingerprints497 проверены до/после build):

| Binary | SHA-256 |
|---|---|
| bench_battle | `3fb95e51ed4b5ea82acb31f8f1ea5c9260438bca9e0fd92a64c2f64dc390ab72` |
| sim-cli | `a8d2a20108eefa043d96914aca9cdc3ecde189c5d0b091d14121b1524c0bd7a8` |
| blast | `7e7a146bf7cfa23097daeef03cf1271a1554e95844744f5e6fc1555ad29afc9b` |

Sidecar содержит base GitHead3c2e8f1 и WIP source fingerprints;
это content provenance, не clean-build/toolchain attestation.

## Обычный развёрнутый полигон

Один after World240 на финальном frozen blast; before — ordinary контроль
321 на прежнем бинарнике, в тот же день/на той же машине. Stock,1×,
physics4/nav8/Rayon4/Compute4,Vulkan/RTX5090,3840×1949,focus=true,
AutoNoVsync, камера/контекст не менялись. GPU/core/OS tracing выключены.
Оба completed/ValidForComparison=true; FPS AcceptanceEligible=false из-за
провала строгого темпа SIM. Это одна timed-пара, не native fixed-tick ABBA.

| Показатель | Before321 | After322 |
|---|---:|---:|
| Mean FPS |36,6|38,9|
| Frame p99, мс |59,849|57,731|
| Frame max, мс |125,738|122,604|
| Тиков за≈240 с |6729|6871|
| Realtime ratio |0,9343|0,9540|
| Mean core мс/фактический тик |21,990|20,757|
| Core мс/тик в180–240 с |26,712|25,169|
| FPS в180–240 с |24,90|25,78|
| Списанный accumulator time, тиков |442,300|300,202|

After по минутам:61,37/38,67/29,75/25,78 FPS. Наблюдаемый core/tick ниже;
строгий FPS A/B анализатор **не принимает** эту пару: обе SIM не realtime,
различаются measured steps и terminal state hash. Это описательное
сопоставление, не квалифицированный причинный FPS gain. Накопительная деградация сохраняется;
30 Гц,120 FPS p99 и165 FPS every-frame не пройдены. After продвинулся
на142 тика дальше и потому содержит больше late state; это не одинаковые
конечные состояния. Hash `846a1e7a9149fabe` относится к6871, прежний
`355099ec2ce34ac0` — к6729. Детерминизм квалифицируется fixed-tick runs,
не сравнением этих terminal hashes. Рост records/backlog между такими
разными tick не трактуется как новая утечка или физическая регрессия.

## Контроль других тяжёлых сцен

Финальный release,physics4/nav8/Rayon4. По одной последовательной паре
before/after; фиксированные тики, все36 нетайминговых полей каждой строки
и финальные hashes совпали. Значения — mean мс/тик, не FPS.

| Сцена | Тиков | Root до→после | Rubble до→после | Hash |
|---|---:|---|---|---|
|4, картечь T2|900|1,922→1,846|0,529→0,494|`39ed0e187213fa88`|
|5, осадная T3|900|2,522→2,360|0,747→0,624|`1fd7abba7eec5963`|
|12, Бастион|4000|3,733→3,497|1,458→1,229|`354e0b04fed1bec1`|
|15, картечная оборона|4000|2,075→1,915|0,901→0,762|`4eb06a9e3e77a598`|
|16, контрбатарея T3|4000|2,479→2,226|1,330→1,090|`3edaedcda5bda080`|

Сцена12 на900 тиках остановилась проверкой самого **before**:
`a side never took spatial damage`. Окно увеличено до4000; оба варианта
прошли исходные проверки стрельбы/повреждений и ledger. Проверка не
отключалась, производственный код ради fixture не менялся.
Режим bench — deployment→ordinary battle, не полный MissionRun evaluator;
это не повторная квалификация всех этапов миссий или всех26 сценариев.

## Проверки корректности

Независимый per-cell/BTreeMap oracle сравнивает новый диапазонный builder
без использования его masks, scatter, rows и scratch. Cases: повернутые
неполные формы, дробные/отрицательные края footprint, top0/65535,64
повёрнутых корпуса, обратный source order, рост/сокращение/reuse буферов,
empty/sparse вход и1/2/4/8 workers.

Существующий oracle всей rubble расширен:8 последовательных входов с
правками земли, корпусами/их voxel roof, yielding-материалами и rest/wake,
масштабом, shape/side/basis, перестановкой и сокращением массива. Он
сравнивает карту по исходному алгоритму, включая cold/warm builders.
Существующий save/hash/restore тест проверяет derived scratch.

Адресные rubble tests:13/13. Полный release SIM:1009 passed,
0 failed,16 ignored (211,51 с). Независимый oracle и save/restore проверки
входят в эти результаты, а не только в отдельный синтетический stand.
Golden600 на1/4/8 physics workers: `635200a9c3fc1530` во всех трёх.
Архитектура12/12, документы3/3; формат и diff проверены.

## Артефакты и предел результата

Локально `out/optimization-322/`: before/candidate/spans profiles и CSV,
первый отвергнутый binary, результаты сборки/тестов и повторных контролей.
Baseline binaries — проверенные `out/optimization-320/after-*`;
до изменения совпадают497 runtime source/data inputs.

Воспроизведение на замороженных binaries (для повторов — новые CSV/log paths):

```powershell
$env:RAYON_NUM_THREADS='4'
./out/optimization-322/after-bench_battle.exe --world --ai-vs-ai --ticks 6000 --physics-workers 4 --workers 8 --profile-rubble --tick-csv out/optimization-322/repeat.csv
./out/optimization-322/after-bench_battle.exe --scenario 12 --ai-vs-ai --ticks 4000 --physics-workers 4 --workers 8 --tick-csv out/optimization-322/repeat-bastion.csv
./scripts/profile-range.ps1 -Seconds 240 -Scenarios world-battle -Output out/optimization-322/repeat-native -SkipBuild -BinaryPath out/optimization-322/after-blast.exe -PhysicsWorkers 4 -NavigationWorkers 8 -RayonWorkers 4 -ComputeWorkers 4 -Maximized
```

Before — те же аргументы с `out/optimization-320/after-bench_battle.exe`.
Release собирался locked/offline; fmt/diff, архитектура и документы
проверены. Итоговый `target/release/blast.exe` пересобран с ADR-747.

План316 обновлён по итогам квалификации. Оптимизация columns
не означает готовый persistent support index, устойчивый drain всех очередей
или достижение every-tick≤33 мс/native165 FPS. Rare1–2-секундный фриз,
30-minute/VRAM soak и материальные5000 не закрываются этим изменением.
