# ADR-598 — индекс только для фактических запросов опоры

Дата:2026-10-06. Исходный main8590c9a, продолжение ADR-597.

Профиль ADR-597 показывает support build mean0.501мс против query0.030мс.
Индекс карты опор строился как индекс полёта: world box по скорости плюс
ячейка по каждой оси для гравитации. Карта спрашивает только box на0.5
мировой единицы ниже. Недостижимые query cells не нужны её контактам.

Private SettledReach::Support описывает именно эти ячейки; Motion
сохраняет прежний swept reach. Геометрия boxes, rounding, порядок
fragment indices и исключение hull riders прежние. Queries сохраняют
skip/from_above и строгую проверку overlap. Выбор более узкого membership
не изменяет cap, материю, импульс, damage, save format или момент обновления.

Differential test сравнивает оба индекса на31/1023/1024/1025 fragments,
workers1/4/8, negative/fractional positions, поворотах, скорости,
resting/flying и наличии hull. Проверяет реальные hits/misses, явную
поддержанную пару, равные frozen boxes и меньший набор cells. Прежние
rubble reference tests включают yielding, terrain/hull edits и restore.

Артефакты:out/fix598. Frozen before.exe собран до правки burst; новый
variant собирается с явным refresh. Изолированный индекс проверяется
отдельно от рабочего дерева с исходными WIP. Во время замеров нет
сборок или тестов; CPU/native timings и diagnostic scopes различаются.
## Корректность и CPU

Изолированный индекс:652 lib passed,4manual ignored;4golden/19save passed,
CLI seed42/ticks1000 hash66041a9f60189841. Новый differential test отдельно
проходит в рабочем дереве. Staged burst совпадает побайтно с export и
исходником frozen variant. Исходные WIP остаются вне коммита.

Frozen before.exe SHA40FCA6190E203511A8651B54E6C32C61B4BCD7585AD47582CA9407F614AE4720;
after.exe SHAAB3F9CAB6AA6D4ED1764F858C64436D5EE492EE8C747B923A71F10DD3A70EEB6.
Два CPU pair: before→after и after2→before2,1800ticks/world60/AI обеих
сторон, physics/nav=8. Каждый hash1f697f865e6f8aa6; все15 нетаймерных
колонок CSV каждого тика совпали. Все12 окон каждой damage-метки,
143wave_progress и48fire_window совпали. Сборки и тесты во время замеров
завершены. CSV/logs/comparison JSON сохранены.

| CPU, мкс | Before1 | After1 | Before2 | After2 |
|---|---:|---:|---:|---:|
| rubble mean |1092.49|1055.37|1097.01|1047.72|
| rubble p95 |1659|1610|1655|1583|
| rubble p99 |1870|1844|1883|1831|
| rubble max |2606|2255|1988|2004|
| tick mean |10250.43|10171.60|10143.35|10133.56|
| tick p99 |15736|15500|15310|15446|
| tick max |20907|20743|20939|21154|

Rubble mean −3.40/4.49%. Tick mean −0.77/0.10%; второй p99/max хуже.
Устойчивый локальный выигрыш не означает исправление tail stalls или
значимый выигрыш полного боя. Другие фазы также колеблются.

## Ограничение первой native серии

Первый candidate native.exe имеет explicit source refresh, а baseline
out/fix597/native.exe — RefreshTrackedInputs. Analyzer законно отклонил
обе пары: different_or_missing:recorded_build_context. Проверка SHA355
старых inputs нашла только ожидаемый изменённый burst, но это не заменяет
требуемую одинаковую политику сборки. Manifests/гейты не переписываются.
Нужен fresh native-tracked.exe с полным refresh для итогового замера.

Ориентировочные FPS106.4→105.2 /105.9→105.8 и maxima37.989→44.818 /
37.865→46.542 не квалифицируют speed comparison. Все runs1800ticks,
tempo≈1×, hash/alive/damage/shots прежние; сами captures валидны.
165FPS и длинные зависания остаются открыты.

## Итоговый native и подробный профиль

Fresh native-tracked.exe SHA
D688E6D89540797E22F904A8C41E566A36F50A52A35E1C1CBC02A097F12C012F.
Все355 перечисленных inputs неизменны через сборку и после неё; по
сравнению с355 baseline inputs отличается только burst. Rust совпадает
с tested index. Это рабочий состав с исходными WIP, не чистая сборка main;
generated/SDK/registry inputs builder полностью не аттестует.

Итоговая ordinary пара camera-before2→camera-tracked проходит analyzer:
eligible=true, matched_recorded, fps_acceptance_eligible=true. Те же
world60,AIvsAI,physics/nav8,compute4,1600×900,AutoNoVsync,Gpu и wall camera
tour. Каждый1800ticks/tempo≈1×; hash/alive/shots/damage совпадают.
Camera11segments,120→121origins,0blocked. Это одна qualified пара;
ранее отклонённые пары её не усиливают.

| Native | Before | After |
|---|---:|---:|
| FPS |105.9|105.6|
| core/step, мс |11.453|11.389|
| p99 interval, мс |27.367|27.460|
| max interval, мс |37.865|41.323|

FPS gain не доказан; core≈−0.55%, tail не улучшен. Narrow membership
оставлен как сокращение лишнего построения с устойчивым локальным CPU
выигрышем; это не закрывает бюджет всего кадра или165FPS.

Дополнительный camera-core60с с CorePhases/RenderTimeline/CpuThreads/
OsPressure сохраняет все1800traced ticks,78806render spans,0lost/incomplete;
Validity=true, AcceptanceEligible=false. Hash и результаты боя прежние.
Camera11segments/107origins/0blocked,stream max15.125мс.
Support build mean0.452мс против0.501 в ADR-597, drop0.017 против0.033;
queries0.029мс почти прежние. Scopes inclusive; leaf/parent не складываются
как CPU. Не сравниваем FPS detailed capture с ordinary runs.

Максимальный rubble parent2.777мс (tick1397), build1.328мс. Прежний10мс
rubble симптом не повторился; нельзя объявить его исправленным.
Пик кадра41.358мс на39.744с: previous Update33.231мс +gap8.127мс.
В предыдущем Update core12.294 вложен в simulation_input16.098мс;
presentation15.661мс. Render atomics следующей строки не атрибутируют
предыдущий frame. Gap не означает чистый wait/idle. Многосекундные
зависания в этом повторе не воспроизвелись и не считаются закрытыми.

Следующий приоритет по общему кадру: WBPERF-002 main-thread tick/publication
и camera streaming/presentation tails; затем motion/debris. Remaining
support build требует отдельного разбора подготовки boxes/query masks,
а не новых parallel queries. P-87 остаётся физической задачей отдельно
от сокращения стоимости индекса.
