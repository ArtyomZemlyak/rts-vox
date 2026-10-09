# Собственные снимки геометрии юнитов для сборки повреждённых моделей

Дата: 08.10.2026. Продолжение EP-02b/03 после
[230](230-engine-world-reset-lifecycle.md), ADR-636. Контроль — frozen 230.
Критерии записаны до изменения кода в
`out/engine-fixes-20261008/owned-unit-geometry/acceptance.json`.

## Проблема и исправление

Witness worker уже работал асинхронно, однако renderer самостоятельно
копировал RangeUnit.body, dents, segment_dents и seg_gone в свой Snapshot.
Кадровый coordinator также напрямую читал authoritative roster, состояние
смерти, revision, removed history и вычислял позиции косметических осколков.
Это оставляло отдельную продуктовую схему чтения вне общей модели движка.

Общий `read_model::geometry` предоставляет два уровня данных:

- View содержит generation/tick, небольшой список revision/kind/dead/anchor/work-cost
  и ограниченную выборку removed cells с готовыми целочисленными позициями.
- Mesh содержит тело одного юнита, тела сегментов, damage ledgers,
  seg_gone, принадлежность стороне и config, принадлежавшие моменту capture.

Оба результата владеют своими данными, имеют Send+Sync, не содержат ссылок
на BlastRange, контроллеры или физические очереди. Изменяемая occupancy
тела и сегментов копируется. Неизменяемые labels/finish/path descriptions
UnitBody сохраняют прежнее Arc sharing. Параметры сценария или его название
не участвуют в выборе этого интерфейса.

View обновляется при каждой публикации с reuse векторов. Tick не служит
кешем: смерть, damage, config и placement на паузе не скрываются. Mesh
запрашивается только для принятой задачи; stale generation, tick, config,
revision, anchor или отсутствующий slot отвергаются до копирования тела.
Generation задаёт owner/adapter; глобальная уникальность независимых
owners этим API не создаётся. Revision предполагает соблюдение обычной
инвалидации при изменении геометрии; полный hidden mutable access ещё открыт.

Повторная проверка выявила прежний пропуск cache key: конфигурация
unit_ablation_resistance/unit_role_resistance_milli влияет на отображение
вмятин через unit_work, но damage revision может не измениться. Revision
модели теперь содержит также эти параметры. Completed и pending mesh key
отличаются от свежего после смены config на паузе; старая задача может
кратко опубликовать прежнюю версию, после чего планируется новая без
ожидания очередного повреждения. Сам physical unit.revision не меняется.

## Ограничение копирования и visual policy

Witness сохраняет прежний лимит двух одновременных mesh jobs и обход
round-robin. Frame publication не клонирует тела всей армии. Старый Snapshot
renderer удалён; mesh build получает готовые данные движка вместе с config.
Coordinator читает View для смерти, удаления сущностей, anchors, revisions,
выброса осколков и допуска новых jobs. Единственные обращения к Scene.sim
в production witness — два вызова общего export/capture.

Cosmetic Sampling задаётся потребителем: прежние stride 97 и максимум 32
на юнит сохранены в renderer. Движок хранит только типизированный запрос
и переносит выборку в собственные records. Cursor содержит generation;
watermarks другого поколения начинают выборку с нуля, отсутствующие
slots тоже, слишком большой cursor ограничивается текущим removed count.
Records идут в прежнем порядке юнитов и source removal order. Cursor после
потребления продвигается на весь removed count, как в прежнем пути.

Выборка существует только при прежнем impact/material_energy_j==0;
она не создаёт физические fragments и не меняет conservation. Координаты
получаются прежним RangeUnit::world_pos. Размер ограничен roster×per_unit;
глобальный бюджет visual publication этим срезом не вводится.

Reset по-прежнему очищает seen/pending/emitted при смене generation до
потребления результатов. Завершённая старая damage revision может быть
показана, пока новая ещё собирается, как раньше. Материалы смерти,
локальные transforms, clipping сегментов, приоритеты и физика прежние.
Полный async cancellation/lifecycle protocol не заменён этим снимком.

## Проверки

Целевой release-прогон: **12 pass, 0 fail**, включая шесть новых geometry
tests и шесть прежних регрессий. Проверены независимость occupancy тела
и сегментов, dents/segment_dents, seg_gone и config; export не меняет save.
На том же tick запросы старых damage/death/config/anchor отвергаются;
свежая публикация сохраняет allocation и видит изменения. Проверены
короткий/пустой roster, layout-less factory и смена вида через общий reset.

Sampling проверен на stride3/per_unit2, oversized/missing cursor,
другом generation, отсутствии impact, обычном material_energy_j и нулевом
per_unit. Source order и world coordinates сохранены; старый View остаётся
неизменным после обновления нового. Отдельный реальный бой идёт 240 шагов:
сборка mesh inputs не меняет ни одного physical hash, restore на шагах
60/200 сохраняет snapshot; требуются реальные shots и removed damage.

Renderer сравнивает индексы, positions, normals, colors, UV, tangents,
finish/segment/turret и local transforms готовых моделей с direct inputs
для всех девяти kinds. Прежние проверки смерти, повреждения пластины и
worker snapshot isolation переведены на общий capture; смерть охватывает
также ArtilleryTurretT2.

Новая регрессия смены стоимости work проверяет изменение видимых colors
вмятин без новой damage revision. Ниже приведены финальные результаты
после исправления ключа. Первую серию до этой правки сохраняет before-work-key
вместе с raw и бинарниками; повторные проверки/замеры вызваны изменением
реализации, не отбором более выгодного результата.

Полный release-прогон финальной версии: **1026 headless/data
pass, 0 fail, 6 ignored**; **162 UI pass, 0 fail,
4 ignored**. В том числе прошли сравнение моделей всех kinds и новая
регрессия стоимости вмятин. Architecture: **5 pass / 4 fail**, ровно
прежние **46** нормализованных диагностик относительно 230, без добавлений.
Четыре старых архитектурных отказа остаются открытыми. Финальные пять
witness tests — подмножество UI suite. `rustfmt --check` и `git diff --check`
прошли. Headless comparison прошёл. Первая native пара не прошла FPS gate;
фиксированное исследование из трёх пар описано ниже.

## Стоимость собственного export

Один отдельный прогон, прогретые повторения на неизменяемом мире:
20 000 refresh View, 20 capture/drop каждого из 60 юнитов, 2000 sampled
refresh. Clock и physical hash находятся в benchmark binary, вне sim library.

| Метрика | Tick0 | Tick3300 |
|---|---:|---:|
| Metadata refresh, mean мкс | 0,208 | 0,204 |
| Metadata logical bytes | 4120 | 4120 |
| Mesh capture, mean мкс | 2,403 | 3,346 |
| Mesh drop, mean мкс | 0,613 | 0,870 |
| Наибольший mesh private payload, bytes | 333628 | 338220 |
| Сумма двух наибольших private payloads, bytes | 667256 | 671872 |
| Sampled refresh, mean мкс | 0,223 | 9,225 |
| Sampled logical bytes | 4120 | 12976 |
| Sampled records | 0 | 369 |

Hash до/после измерения сохранён: early **eef062059c01815b**, late
**60bc6fb338a5ed21**. Sampled вариант искусственно воспроизводит retained
removals с cosmetic policy и не является обычной stock нагрузкой.
Метрики не сравнивались с direct-copy microbenchmark прежнего renderer;
они показывают цену нового контракта, не speedup. Private payload pair
не равен RSS двух jobs: shared данные, spare capacity, BTree nodes
и готовые render meshes исключены.

## Полный stock бой и replay

Три последовательных прогона по 3600 тиков: frozen 230, кандидат,
кандидат с late restore 3300→3600. Все **25 нетайминговых полей каждого
тика**, исход боя, выстрелы, повреждения и hash **5dd0c782d2b2f557**
совпали. Late replay завершился тем же состоянием.

| Прогон | Mean tick, мс |
|---|---:|
| Frozen 230 | 13,304166 |
| Новый | 13,346843 |
| Новый с late replay | 13,398646 |

Это проверка сохранения физической нагрузки, а не доказательство speedup:
сам geometry export отсутствует в stock headless кадровом пути. Его
production consumer проверяется UI tests и ordinary native comparison.

## Ordinary native

Шесть запусков окончательного сравнения, три пары по 120 с, 1600×900, 165 FPS target,
physics/navigation по восемь workers, compute четыре. Builds/tests во время
замеров не выполнялись. Все шесть запусков получили ValidForComparison=true,
AcceptanceEligible=true, MeetsSimulationRate=true, по 3600 тиков и hash
**5dd0c782d2b2f557**, без причин исключения.

Первоначальный протокол предусматривал одну пару. Она не прошла FPS gate:
70,1→67,7 FPS, −3,424% при допуске 3%; p99 33,614→33,762 мс, +0,440%.
Отказ сохранён в qualification.initial-failed.json, а measure.ps1 завершился
с exit1. Код после этого отказа не менялся.

Один и тот же frozen control дал 67,1 FPS в предыдущей серии до изменения
cache key и 70,1 FPS в первой финальной паре: разброс 4,47% больше допуска.
Предыдущий кандидат в финальное сравнение не включён. Суммарное время
unit_mesh первой финальной пары составило 489,9→487,8 мс, не объясняя
разницу FPS; это само по себе не исключает косвенную регрессию.

До дополнительных запусков записан acceptance-investigation.json: ровно
две дополнительные пары, порядок after2→control2 и control3→after3.
Порог 3% сохранён и применяется к медианам всех трёх запусков каждого
бинарника. Первая неудачная пара включена; исключений и дальнейших
повторов для улучшения результата нет. Изменение количества замеров
после первого отказа — предел этой квалификации, а не прохождение
первоначального однопарного критерия.

| Пара | Порядок | Control FPS | Новый FPS | Control p99, мс | Новый p99, мс | Парный gate |
|---|---|---:|---:|---:|---:|---|
| 1 | control→after | 70,1 | 67,7 | 33,614 | 33,762 | не пройден |
| 2 | after→control | 68,5 | 70,2 | 33,883 | 33,342 | пройден |
| 3 | control→after | 68,0 | 70,5 | 33,699 | 33,268 | пройден |

Медианы по трём запускам каждого бинарника:

| Метрика | Frozen 230 | Новый |
|---|---:|---:|
| Mean FPS | 68,5 | 70,2 |
| p95 кадра, мс | 31,334 | 30,960 |
| p99 кадра, мс | 33,699 | 33,342 |
| Максимальный кадр, мс | 50,353 | 49,741 |
| Simulation realtime ratio | 0,9996 | 0,9996 |

Gate FPS≥97%, p99≤103% контроля пройден по медианам фиксированного исследования.
FPS +2,482%, p99 −1,059%. Три коротких пары при наблюдавшемся
разбросе контроля не доказывают устойчивого ускорения и не гарантируют
отсутствия меньшей регрессии. Цель 165 FPS остаётся открытой.
`native-investigate.ps1` и final qualification завершились с exit0;
`qualification.json` связывает source hashes, бинарники, тесты, export
metrics и оба вида battle measurements. Native raw всех шести запусков —
в native-control1/2/3-final и native-after1/2/3-final. Предыдущая серия до
исправления cache key сохранена отдельно в before-work-key.

## Оставшиеся readers и следующий шаг

| Участок | Состояние после 231 |
|---|---|
| Witness damage meshing и его coordinator | Metadata, cosmetic samples и mesh inputs через общий owned export |
| Manual gun geometry / gun_art | Ещё читает физическое тело напрямую |
| Moving poses, history, occupancy/shadows | Полный publication контракт ещё не перенесён |
| Fragments, bodies/wreck и collapse geometry | Ещё требуют отдельных bounded payloads и lifetimes |
| Fire, projectiles, shields/effects | Полный event/state export ещё открыт |
| Terrain streaming, surface patches, minimap background | Колонки/edits и surface state ещё читаются напрямую |

Ближайший срез — ограниченный terrain export для column/row и изменений
surface, с generation/tick и проверкой staging repairs при скачках камеры.
Toroidal GPU packing, material colors и выбор окна остаются задачами
renderer; движок отдаёт целочисленные данные. Перед переносом owner нужен
полный перечень readers/writers, общий coherent publication и измерение
copy/free/memory позднего мира. Необходимость прочих geometry/effect
срезов не исчезает после исправления witness.

EP-02/03, WBPERF-002/020 остаются частичными. Physical debt, release/wait
dependencies и миллионные очереди этим переносом не меняются. Ни устойчивое
ускорение боя, ни 165 FPS не объявляются достигнутыми.

## Воспроизводимость

409 входов frozen 230 проверены по SHA-256 до изменений. Кандидат содержит
412 входов: четыре существующих Rust-файла изменены, добавлены geometry
module, его tests и export benchmark. Locked offline release build
инкрементальный, без принудительного refresh исходников или независимой
SDK rebuild; этот предел происхождения отмечен в паспортах. Критерии,
backups, scripts, build/measurement logs и qualification находятся в
`out/engine-fixes-20261008/owned-unit-geometry/`.
Данные, зависимости, physical snapshot, portable checkpoint и golden files
не меняются. Протокол: три stock AI боя по 3600 тиков, late replay 3300→3600,
все 25 per-tick нетайминговых полей и hash 5dd0c782d2b2f557; первая ordinary
native пара по 120 с с valid/eligible/tempo и gate FPS≥97%, p99≤103% контроля.
После её отказа — фиксированное исследование трёх пар с медианным gate,
той же границей 3%, сохранением отказа и всех шести исходных запусков.

Отдельный export benchmark измеряет ранний и tick 3300 мир: refresh View,
on-demand mesh copy и drop, logical payload и две наибольшие mesh copies.
Sampled variant принудительно включает cosmetic impact только внутри
диагностики; исходный config/impact восстанавливаются и physical hash
проверяется. Это отдельная visual нагрузка, не изменение stock baseline.
Private payload исключает Arc labels, spare capacity и BTree node overhead;
он не является измерением process RSS или полной памяти поколений.

SHA-256 frozen бинарников:

| Бинарник | SHA-256 |
|---|---|
| 230 bench | `5E66F62FB7C47A6FF88713960CBDBB4AB4809C30BF6348D0E41D40D2868BF2E7` |
| 231 bench | `A40292A9F8EB148B0EBE0B8BF019CF5E83AA1569730388CB82AA4CEDDAC6D7A4` |
| 230 native | `2B5019DDA27D60E71592B186322FA8BAC5FDEF94DB15991F00DCDB0BB116C78C` |
| 231 native | `7DDBA8E7B92B90EDE035A923BDB5633DA8722C028701112203C10EF443491C11` |
| 231 export benchmark | `1D3BCAD4291BCF6098736696CDDA624A35F01C45AD76812E1F96EBF4C0EAEB88` |
