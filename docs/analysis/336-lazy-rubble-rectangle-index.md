# 336. Дерево прямоугольных запросов завалов по первому чтению

Дата:2026-10-19. База:`main cbdbb51`; единственная очередь —
[316](316-optimization-delivery-plan.md). Статус: performance gate пройден в двух ordinary парах;
полная qualification ниже. Решение ADR-753. Это не persistent membership фрагментов04a.

## Основание и границы профиля

Диагностический saved World tick6000,hash `21bef878463f364c`,physics4/
navigation8/Rayon4,128 шагов до6128/hash `d925ec22d62f7b00`.
Before текущего main пересобран специально; это не старый executable327.
Trace включает Mutex/Instant worker observers: времена inclusive и
не являются ordinary headless или FPS. Суммы параллельных leaves нельзя
прибавлять к parent. Rubble parent5,347мс,columns1,866; внутри columns
gather_dispatch0,512 иmerge0,650мс. Остаток≈0,704мс до изменения не
размечен отдельно; считать его доказанной стоимостью одного Index нельзя.

Проверка кода выявила eager `Rubble::from_sorted`: перед сравнением карт
строится дерево границ/max, хотя равная временная карта затем выбрасывается.
В обычном before-b335 у2344/3600 поздних тиков changed_columns/full_builds
равны нулю — косвенный признак равной карты, не счётчик построений дерева.
Отдельный fresh-scratch snapshot запрос показывает336715 колонок,
69002 fragment records,39 hulls. Это одна карта, не вся матрица сцен.

## Общий алгоритм

Канонические отсортированные колонки рассчитываются полностью как прежде.
Derived rectangle Index хранится в собственном OnceLock Rubble и строится
синхронно при первом touches/upper_in. Поиск до512 колонок сохраняет
старый прямой путь. Point get,iteration,equality и discarded equal maps
не требуют дерева. Canonical refresh удерживает прежний объект равной
карты; это прежний ADR-743, теперь без eager построения временного Index.

Первый читатель строит полный прежний Index по текущему canonical slice;
другие читатели ждут его завершения. Нет старого ответа, пропуска опор
или отставания. Insert/replace/remove сбрасывают rectangle и coarse кэши;
collection начинает холодным. Clone до и после первого чтения корректен.
Tree geometry, leaf size, traversal и число leaf reads не меняются.

Мир, hulls, поддержка и колонки пересчитываются заново; новый порядок
касается только чистого производного дерева. Квоты, материя, оружие,
SIM frequency, collision/navigation того же тика и snapshot/hash прежние.
Холодная изменённая карта может перенести цену Index в motion/query;
уменьшение одной rubble фазы само по себе не является performance gain.
Прежние root/motion/rubble измеряются вместе.

Новый PhysicsTask `physics_rubble_publish` выделяет создание Rubble
из готовых колонок; включён в opt-in bench parent profiler и balanced
SIM traces. Часы остаются снаружи SIM. Lazy Index construction внутри
nav query пока не имеет собственного wall span: его цена входит в
motion/root. Публикация — часть columns, inclusive spans не суммируются.

## Адресные проверки

Debug nav13 и sim-core rubble15 прошли. Новые тесты проверяют отсутствие
Index после get/iteration/equality/cold clone, один готовый Index после
первого чтения, warm clone, замену maximum, удаление и порог512.
Восемь одновременных readers получили один и тот же Index и точные ответы.
Существующий cell/full-scan oracle покрывает отрицательные/крайние i32,
нулевую высоту, insert/remove/collection и переходы размеров.
Прежний local-query тест сохраняет≤64 прочитанных колонок при росте
истории до262144. SIM tests проверяют полноценные карты после изменений
опор/hulls,маски bounds, restore и сброс scratch; новый publish span balanced.

## Performance приёмка и evidence

Ordinary World6000,physics4/nav8/Rayon4;before frozen335,after отдельный
probe336. Поля36 каждого тика и hash сравниваются до приёмки.
Source/data fingerprints497; runtime/test/observer изменения ограничены
vehicle.rs,vehicle/rubble_index.rs,vehicle_clearance_tests.rs,
burst.rs и bin/bench_battle/rubble.rs. Новых зависимостей нет.
`out/optimization-336/` хранит before/probe executables, диагностический
профиль, map-before.log, адресные tests, source-inputs и CSV/log/analyze.py.
Release lib: nav127 и sim-core1012 прошли,16 SIM tests ignored, ошибок нет.
Остальные проверки и native qualification приведены ниже.

## Повторные ordinary контроли и подробная диагностика

World6000,physics4/nav8/Rayon4. Все36 игровых полей каждого тика и hash
`21bef878463f364c` совпали. Перед after diagnostic выполнялся ordinary:
диагностические clocks и Mutex не входят в ordinary времена.

| Метрика, мс | Before-a | After-a |
|---|---:|---:|
| Root mean |16,26617|15,87859|
| Root p99 |46,111|44,618|
| Root max |105,525|104,704|
| Late root |18,27649|17,65773|
| Late rubble |4,76448|4,02277|
| Late motion |4,75878|4,92540|
| Late debris |5,11605|5,13153|

Root дешевле2,38%,late root3,39%;rubble−15,6%,motion **дороже3,5%**.
Это ожидаемый перенос cold Index работы плюс экономия unused builds;
уменьшение только rubble не выдаётся за ускорение всего движка.
Повтор в обратном порядке подтвердил выигрыш; результаты ниже.

Диагностический after128:тот же6128/hash `d925ec22d62f7b00`.
Rubble5,347→4,571мс,columns1,866→1,088;motion5,293→5,459.
Новый publish≈74нс/tick; это стоимость lazy object construction, а не
стоимость индекса, перенесённая в query. Прежний неразмеченный остаток
не подменяется задним числом прямым before замером publish. Mutex/worker
overhead и короткое окно не позволяют считать эту пару FPS qualification.


Вторая пара выполнялась after-b → before-b. Средние не взвешиваются по
длительности: все запуски имеют одинаковые6000 шагов; late окно2401..6000.

| Метрика, мс | Before-b | After-b | Среднее before | Среднее after | Изменение |
|---|---:|---:|---:|---:|---:|
| Root mean |16,41376|15,91598|16,33997|15,89729|−2,71%|
| Late root |18,53405|17,70665|18,40527|17,68219|−3,93%|
| Late rubble |4,85593|4,04797|4,81021|4,03537|−16,11%|
| Late motion |4,80463|4,94229|4,78171|4,93385|+3,18%|
| Late debris |5,14254|5,13369|5,12929|5,13261|≈0%|
| Root p99 |43,803|42,870|—|—|—|
| Root max |105,915|104,466|—|—|—|

Все четыре запуска имеют одинаковые36 не-временных полей каждого тика
и конечный hash `21bef878463f364c`. Это ускорение всей SIM на2,7%,
а не16% всего движка. Motion дороже из-за переноса нужной холодной сборки;
root подтверждает чистую экономию. Every-tick33мс по-прежнему не выполнен.


## Итоговая сборка и проверки

Locked/offline release build bench_battle/sim-cli/blast прошёл. Probe и
итоговый runtime совпадают; различие vehicle.rs — только уточняющий doc
comment. Проверены497 source/data fingerprints до/после build. Это WIP
над cbdbb51, не clean-build/toolchain attestation. SHA256 итоговых файлов:

- bench_battle: `2749358f84a9b5ddf8dd2d0007442ef56d62932df90b1fba823bccd8f048b06b`.
- sim-cli: `c046ae4b9b46ff9be87c4dd2878b9258da4571b8322ac2578abea515dcab12b0`.
- blast: `4bf302afc83f6a445d9b0a9307238c19c451e16f65fa31d362fae13ab57216fb`.

Release nav127,sim-core1012 (16 ignored),bench_battle7 прошли;
архитектура12 и docs3 прошли через прежние cached harness executables,
читающие актуальные source/docs; определения harness не менялись.
Fmt/diff checks прошли. Docs validator:351 Markdown,1300 local links,
новых/прежних битых ссылок0,35 shared texts сохранены,73 исторических
дубликата по-прежнему заменены ссылками. Нового плана-дубликата нет.

Итоговая сборка отдельно прошла World6000:mean15,82521мс,p9942,497,
max105,592;late root17,59947,rubble4,02958,motion4,89933,debris5,14104мс.
Все36 игровых полей каждого тика совпали с before-b,hash
`21bef878463f364c`. Этот одиночный qualification не новая парная оценка
ускорения. SIM600 threads1/4/8:все `635200a9c3fc1530`.

Пять deployment сцен,AI-vs-AI,physics4/nav8/Rayon4. Это обычные бои,
не MissionRun/проверка всех26 миссий. Все36 игровых полей каждого тика
и hashes совпали. Mean root:

| Scene | Ticks | Before,мс | After,мс | Изменение | Hash |
|---|---:|---:|---:|---:|---|
|4|900|1.68892|1.69734|+0.50%|`39ed0e187213fa88`|
|5|900|2.18863|2.22208|+1.53%|`1fd7abba7eec5963`|
|12|4000|3.24180|3.13524|-3.29%|`354e0b04fed1bec1`|
|15|4000|1.75355|1.68699|-3.80%|`4eb06a9e3e77a598`|
|16|4000|2.01003|1.86373|-7.28%|`3edaedcda5bda080`|

Короткие4/5 повторены в обратном порядке (after→before),36 полей/hash
также прежние:4 —1,67563→1,67113мс (−0,27%);5 —2,16605→2,18521мс
(+0,88%). Сцена4 не показывает устойчивого выигрыша; у5 небольшой
повторяемый регресс около0,02мс. Решение принимается за устойчивую экономию
на большой истории завалов; универсальный выигрыш во всех сценах не заявлен.
При следующем motion-профиле сцена5 остаётся контрольной.

## Полигон в развёрнутом окне

Ordinary after240,stock,RTX5090/Vulkan,3840×1949,scale1,focused=true,
Windowed/AutoNoVsync,view1/zone6,overlays[true,true],drawer0,static camera;
physics4/nav8/Rayon4/compute4. Другие builds/tests/analyses во время capture
не запускались. Native manifest SHA256 совпал с frozen/target blast.

- Mean42,5FPS;p9540,668мс,p9954,618мс,max122,060мс.
- 6993 шага,core18,78972мс/step,ratio0,9709564.
- Final debt28,999999 ticks,dropped178,213312 ticks.
- Конечный hash `d6788548661f54bb`; финальный tick иной, чем headless6000.
- Evidence valid=true;FPS acceptance=false;realtime=false.

Это одиночный after с иным конечным tick/state, не matched native FPS A/B.
Рост FPS относительно прежних одиночных after не доказывает причинность.
Отдельные кадры всё ещё>100мс; отсутствия редких1–2сек фризов за пределами
240сек не доказано. Every-tick33мс,30Гц без списания долга и165FPS
остаются открыты. GPU,VRAM/RSS и полная матрица камер не квалифицированы.

Снимки native показывают77592 fragment records и felling_pending6610699
в конце (это backlog counter, не число живых деревьев). Ускорение Index не
исправляет очередь валки03. Полный fresh rebuild поддержки/membership
04a и active IDs debris05a также остаются открыты. В итоговом позднем
headless окне debris5,141,motion4,899,rubble4,030мс:следующие кандидаты
нужно выбирать по этим расходам и корректности, не по старому rubble7мс.

## Воспроизведение и расположение evidence

`out/optimization-336/` локальный и gitignored:CSV/log,full release tests,
оба repeat,diagnostic128,source/probe fingerprints,production-summary,
scenario-summary/scenario-repeat-summary,doc-validation,evidence-verification,
artifacts SHA256 inventory и native-after240. Baseline frozen335 —
`out/optimization-335/after-bench_battle.exe`; исходники main cbdbb51.
Before diagnostic snapshot executable пересобран до runtime336 изменений.
Saved snapshot: `out/performance-audit-20261016/headless/18.bin`;
219018038 bytes,tick6000/hash21bef878463f364c. Повторный detailed128 имеет
6128/hashd925ec22d62f7b00. Скрипты анализа проверяют, а не только печатают
совпадение игровых полей/hash. Сырые данные не отправляются в git;
таблицы/ограничения/команды в этом отчёте доступны вместе с кодом.

```powershell
$env:RAYON_NUM_THREADS='4'
./out/optimization-336/after-bench_battle.exe --world --ai-vs-ai --ticks 6000 --physics-workers 4 --workers 8 --tick-csv out/optimization-336/production-after.csv
./out/optimization-336/run-scenarios.ps1
./scripts/profile-range.ps1 -Seconds 240 -Scenarios world-battle -Output out/optimization-336/native-after240 -SkipBuild -BinaryPath out/optimization-336/after-blast.exe -PhysicsWorkers 4 -NavigationWorkers 8 -RayonWorkers 4 -ComputeWorkers 4 -Maximized
C:/Python314/python.exe scripts/analyze-range-profile.py out/optimization-336/native-after240 --require-valid
C:/Python314/python.exe out/optimization-336/verify-evidence.py
```

Запуски строго последовательные. Повторный capture с тем же output
перезаписывает прежний evidence:для нового замера нужен новый каталог.
