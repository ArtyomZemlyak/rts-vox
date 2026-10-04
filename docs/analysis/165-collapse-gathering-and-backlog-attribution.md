# ADR-571 — сбор больших обломков и атрибуция очереди обрушений

05.10.2026. Scope: WBPERF-028/P-73 частично; изменения физического
результата, оружия, лимитов и формата снимка не предполагаются.

## Найденные затраты и ограничения

BlastRange копировал BTreeMap последних правок каждой клетки в Vec перед
CollapseLayer, затем передавал тот же массив plate::fracture. При поздних
2286934 записях и sizeof(Edit)=20 это45738680байт временной копии.
Главная блокировка очереди — работающие waves/debris claims/body source
removal; это ownership guards, а не условие наличия свободных частиц.
Удалить их без нового протокола нельзя: возможен повторный перенос клеток.

CollapseLayer::detach_unanchored перечислял каждую клетку каждой связной
группы до проверки max_body_cells. Большая группа, которая заведомо
не поместится в тело, строила полный состав и затем выбрасывала его.
Линейный поиск root в списке повторялся для каждого run.

## Реализовано

- apply_iter_observed принимает полный ordered batch; прежний slice API
  делегирует ему. Плиты получают cloneable iterator того же batch;
  очередь очищается после обеих проверок. Большой Vec отсутствует.
- Root→slot BTreeMap не задаёт обход: Vec групп по-прежнему сохраняет
  первое появление в chunk/run order. Допустимые клетки и IDs неизменны.
- Перед разворачиванием mask проверяется count_ones против остатка
  max_body_cells. Переполнение освобождает префикс и помечает всю группу
  как too_big. Следующие runs только учитывают candidates. Никакая часть
  отказанного тела не снимается; соседняя допустимая группа не блокируется.
- CollapseWork вне snapshot/hash: queued на входе фазы, submitted,
  blocked_by, blocked_ticks, gather_candidates, gather_buffer_peak.
  Bits0/1 означают waves/claims. Только когда они оба нули, проверяются
  source digs (bit2); диагностика не навязывает дорогие лишние queries.
  blocked_ticks — непрерывное ожидание в этом runtime, не возраст клетки,
  после restore 0. Timing находится только в bench callback, не в sim.
- CSV дополняется колонками; прежние позиции сохранены. collapse_us
  суммирует неперекрывающиеся completion intervals включая вставку правок.

## Квалификация complete batch

Frozen before-qualified/after-qualified, одинаковый fixture setup,
RAYON_NUM_THREADS=8. Setup/serialization/logging исключены из измерения.
Каждый fixture повторён3 раза, не статистическое доказательство всех хвостов.

| fixture | before max call,мкс по repeats | after max call,мкс | hash |
|---|---|---|---|
| unsupported slab1048575 cell,cap 4096 |3670,3672,3607|851,733,749|36035c24a6633511|
|2286934 real ground digs|37536,39044,37845|39072,38229,39669|6d80fcf74e5ff1a3|

Для slab после изменения counted candidates 1048575, buffer peak 4095.
Состав большого отказанного тела сохранён в мире. Старый код собирал
все 1048575 координат (≈12MiB), новый освобождает префикс; сокращение
времени этого fixture подтверждено, whole battle gain не заявляется.

Для полного ground batch копия45738680байт устранена, CPU gain не
доказан: ingestion/relabel всё ещё39мс и не имеет общего tick budget.
Физические снимки before/after совпадают для обоих fixtures.
Первый недогруженный backlog fixture ограничился materialization cap;
его `before/after-backlog.log` исключены из доказательства. Исправленный
стенд требует `queued() == 0` и ровно 2286934 applied edits перед измерением.
Он повышает materialization budget только во время подготовки fixture,
не меняет production RON, и не объявляется обычным game tick.

## Длительный world AI-vs-AI

По одному isolated7200tick run before/after,60 юнитов, nav 8 / physics 8.

| CPU tick,мкс | before | after |
|---|---:|---:|
|p50|17702|17770|
|p95|22168|22089|
|p99|24091|23964|
|max|61856|54367|

Обычный tick почти тот же; единичный лучший max не означает исправление
редких stalls. Итог обеих версий95d8e2ad7d998b62, alive 22/24, bases 100/100,
Fighting. hash включает авторитетный снимок, материю и очереди.

Диагностика: очередь 2383908; submitted max 7393, последняя непустая
submission tick 167; blocked_ticks 7032. На7200тиках mask 0: 108, mask 1: 3,
mask 2: 1011, mask 3: 6078. Это постоянные overlapping waves/claims,
не доказательство зависимости от одного конкретного оружия. Body source
probe при cheap blockers не выполняется. Максимальная текущая фаза
collapse 2247 мкс включает backlog insert, большого дренажа в этом run нет.

Поздний snapshot 6900, 185597699 байт, продолжение до 7200 на physics/nav 8
и 1: оригинал/restore/final одинаковы95d8e2ad7d998b62. Runtime diagnostics
не сериализованы; отдельный тест проверяет invariance save/hash.
CSV runs и replay runs имеют разные режимы наблюдения, их timing не
используется для вывода о равномерности CPU либо worker speedup.

## Проверки и воспроизведение

Добавлены реальные cross-chunk tests: большой отказанный slab рядом
с маленьким падающим телом; streamed/slice batch дают одинаковые save
байты графа, мира, тел и очереди transfer во время падения. Прежний тест увеличения
cap разрешает группе падать после отказа. Существующие golden/replay,
plate fracture, matter conservation и range support tests сохраняются.

Команды headless: bench_battle --world --ai-vs-ai --ticks 7200
--workers 8 --damage-trace --tick-csv path; --replay --snapshot-tick 6900
с workers 8/1 и соответствующим RAYON_NUM_THREADS; --collapse-stress
oversized/backlog. Фактические CLI требуют пробел между флагом и числом.
Raw logs/CSV/comparison.json/build manifests — out/fix571. Tested shared
checkout содержит 13 исходных WIP, они исключены из этого коммита; это
не clean-main native FPS qualification.

## Осталось

WBPERF-028/P-73 открыты: bounded incremental ingestion, consistent support
relabel/resolve и plate invalidation, работа при непрерывном огне, claims
и source ownership, finite total candidate/detach budget и длительная
qualification с quiet tail. Нельзя разрешать граф после произвольного
prefix, когда World уже применил весь batch: topology может быть смешанной.
P-76 flying-pool admission/head-of-line blocking остаётся. WBPERF-002/027,
rare multi-second stalls, native frame tail и 165 FPS не закрыты этим этапом.
Новые UI capture, видео и превью не требуются и не создавались.


Финальные проверки: 837 passed в release sim-core/voxel/modding lib+tests,
плюс 8 headless play debris tests; всего 845 passed, 0 failed, 7 прежних
ignored. Первоначальный неверный render filter дал 0 tests; его не
учитываем, фактический запуск debris_tests сохранён в
render-debris-qualified.log. Полный workspace/rare native stalls этим
набором не объявляются квалифицированными.

Frozen battle before.exe SHA256
154E4FDDA89C49E065F2C93EFA7B34ED2FE9AEA230BB6F764B00BDFF5396B1A7;
battle after.exe SHA256
831ADC83A0F9DAA3D5FE55DA63FAE20EFE9F0CE6927737A2540D808EFF0BC8B5.
Последний after-qualified.exe SHA256
D53B3CA756444DF49E023A66675D4373C8497F7C4CB4A50A9AB3FB5B48CB4D01.
Между after/after-qualified изменился только bench_battle fixture setup,
все входные Rust/RON файлы движка идентичны (проверено manifests,
battle-harness-provenance.json). Это позволяет использовать длительный
бой как проверку тех же engine inputs, не выдавая старый executable
за новую сборку. Native build provenance сохраняется отдельно.

Native target/release/blast.exe совпадает с out/fix571/native.exe и
manifest SHA256 770B4DB4D254C9D842BEF06EC68F40F23B53D1C72EF92DCE4078AAE0C0DB9900.
Полигон пересобран; UI/native frame capture не выполнялся.
