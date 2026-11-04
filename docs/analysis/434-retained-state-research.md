# 434 — Retained debris: повторный разбор и внешнее исследование

1 ноября 2026. Дополнение к [сквозному аудиту 427](427-engine-performance-review.md).
Это исследование архитектуры, не заявленный прирост FPS и не ADR.
Последующие [счётчики 435](435-retained-work-counters.md) уточняют A1/A2:
нужен persistent полный `Settled`, а обычные transfers обязаны сохранять
merge-aware memo действительным, без полного rebuild.

## 1. Вывод после повторного разбора

Прежний главный вывод отчёта подтверждён. Проблема не в выборе решений ИИ и не
только в цене одного вокселя: каждый тик заново обслуживается история, которая
к тику 6000 достигла 945 938 фрагментов. На поздних 300 тиках `Settled` стоит
47,193 мс, merge groups 28,233 мс, wake 21,053 мс, merge keys 13,261 мс.

Микрооптимизации 819–824 полезны, но не меняют эту зависимость. Отклонённые
sorted Vec, scratch `selected`, singleton filter, active-list translate и due-mask
также показывают: перестановка контейнеров внутри полного прохода не заменяет
изменение жизненного цикла данных.

**Рекомендуемое главное направление — persistent, инкрементально обновляемые
индексы спящего состояния с точной прежней семантикой запросов.** Целевая
стоимость спокойного тика:

`O(added + moved + woken + due + changed cells)`, а не `O(retained)`.

Это не обещание константного тика: массовое разрушение, большой dirty set и все
движущиеся фрагменты закономерно возвращают линейную работу. Полный builder
остаётся fallback и oracle.

## 2. Что видно непосредственно в нашем коде

`Settled::around_bounds_into` сейчас очищает ячейки, вычисляет мировые boxes для
всей истории, делает полный `selected`, затем строит списки ячеек заново
(`crates/sim-core/src/burst.rs`, 9717–9857). При этом спящие фрагменты должны
оставаться доступными контактам: просто исключить их из физики нельзя.

`step_fragments` каждый тик заново вычисляет join geometry и заполняет
`Memo::current` по всем пригодным фрагментам (7284–7304), даже если changed keys
потом отбрасывают почти всю работу. После ADR-821/823 следующий шаг — хранить
membership и группы, а не ускорять очередной полный контейнер.

Текущий `want` — часть семантики, а не лишняя оптимизация. Полный пространственный
индекс без точной проверки `want` может вернуть контакт, которого прежний builder
не допускал. Именно поэтому y-superset из пробы 424 был неверен.

## 3. Что решают внешние движки и игры

### 3.1 Box2D — наиболее близкий архитектурный ориентир

[Box2D Simulation](https://box2d.org/documentation/md_simulation.html):

- помечает только переместившиеся части broad-phase tree и ищет новые overlap;
- хранит contact pairs между шагами, вместо пересоздания всего мира;
- поддерживает острова и при засыпании переносит их данные из горячего массива;
- сохраняет детерминизм между числами потоков через creation order тел, форм,
  joints и упорядоченные события.

Для нас это прямое подтверждение трёх решений: dirty updates, persistent
membership и active/cold storage. Стабильный ID должен управлять индексом, но
канонический порядок контактов остаётся порядком фрагментов, как creation order
у Box2D. Переносить сам Box2D или его float solver не предлагается.

### 3.2 Jolt — спящее тело не исчезает из мира

[Jolt Architecture](https://github.com/jrouwe/JoltPhysics/blob/master/Docs/Architecture.md)
описывает острова сна: спящее тело не участвует в движении, но сохраняет
возможность обнаруживать контакты с активными телами. При удалении соседних тел
нужное пробуждение выполняется явно.

Для нас `resting` должен означать «нет интеграции и ненужных обновлений», а не
«нет коллизий». Пробуждение от изменившейся опоры должно доходить через
зависимости stable IDs и областей; снятие флага `resting` само по себе проблему
не решает.

### 3.3 PhysX — sleeping-friendly broadphase и двухуровневая группировка

[PhysX Rigid Body Collision](https://nvidia-omniverse.github.io/PhysX/physx/5.5.0/docs/RigidBodyCollision.html)
указывает, что SAP особенно хорош при большом числе спящих объектов, но дорог при
массовом движении и вставках. `PxAggregate` представляет пространственно близких
актеров одним элементом broadphase, оставляя внутренние проверки отдельным уровнем;
локализованные debris также можно вставлять порциями для амортизации.

Вывод не «взять SAP любой ценой», а использовать persistent spatial membership и
двухуровневую группировку куч. Aggregate сам по себе не объединяет материю и не
устраняет narrow-phase. При большом dirty set нужен rebuild/bulk fallback.

### 3.4 Teardown — локальные voxel volumes, а не тело на каждый воксель

В интервью [Game Developer о технологии Teardown](https://www.gamedeveloper.com/design/how-beautiful-voxels-laid-the-way-for-i-teardown-s-i-heist-y-framework)
Деннис Густафссон описывает тысячи небольших локальных voxel volumes вместо одного
мирового объёма; каждый обломок является отдельным локальным объёмом, а CPU
выполняет voxel-vs-voxel collision.

Это поддерживает локальные точные тома и иерархию «pile → shape/cells». Это не
доказывает пригодность чужого solver или конкретного размера кучи для нашего боя.

### 3.5 OpenVDB — полезна разреженная обработка, а не модное дерево

[OpenVDB FAQ](https://www.openvdb.org/documentation/doxygen/faq.html) описывает
иерархические блоки, compact bit masks активных значений и tiles для однородных
областей. Библиотека предназначена для sparse data и sparse computations, но сама
FAQ подчёркивает, что выбор зависит от layout и benchmark.

Для нас полезны локальные страницы, masks и homogeneous summaries. Добавлять
OpenVDB или заменять наши bit volumes деревом только ради слова sparse не нужно:
материалы, paint, повернутая геометрия, ledger и порядок контактов не сводятся к
одному значению tile.

### 3.6 NVIDIA Blast — чужой стандартный приём нам запрещён

[Blast Settings](https://docs.nvidia.com/gameworks/content/gameworkslibrary/blast/1.1/authoring_docs/BlastUe4_BlastSettings.html)
предлагает Debris Filter: удалять мелкие/удалённые фрагменты по времени жизни,
а также сокращать support graph с потерей детализации.

Это типичный игровой способ ограничить нагрузку, но он нарушает наши требования
сохранения материи, геометрии и механики. Не применять таймер удаления, fade-out
с потерей массы, coarsening или упрощение опор. Blast остаётся примером того, что
нам нужен более дорогой точный lifecycle, а не повод скопировать culling.

### 3.7 Volumetrics — отдельная независимая оптимизация

[SIGGRAPH 2014, Bart Wronski](https://bartwronski.files.wordpress.com/2014/08/bwronski_volumetric_fog_siggraph2014.pdf)
и [технический разбор Godot](https://godotengine.org/article/fog-volumes-arrive-in-godot-4/)
используют ограниченный froxel buffer вместо полноценного volumetric solve на каждый
экранный пиксель. Для нашего gas это поддерживает P1: empty-space hierarchy и
low-resolution/depth-aware path. Но свежих GPU passes нет, поэтому GPU-работа не
должна оттеснять доказанный P0 SIM.

## 4. Рекомендуемая архитектура P0-A

### A0. Сначала измерить инвалидацию, а не написать ещё один кэш

Добавить diagnostic-only счётчики за тик и по окнам:

- resting/flying/due/asks и фактически touched IDs;
- added/woken/merged/removed, pose/geometry/material/key changes;
- `want` cells, selected fragments, построенные cell entries, candidate hits;
- число dirty hulls и фрагменты в их old/new swept boxes;
- размер dirty set, fallback rate, bytes read/write и capacity основных массивов.

Нужен профиль раннего, позднего и drain-стенда. Счётчики не входят в hash.

### A1. Persistent `Settled` с точным query gate — первый прототип

Хранить производный индекс между тиками:

1. Запись по stable `FragmentId`: канонический slot, frozen pose/bounds, cell range,
   resting/rider state и версия геометрии. Slot может меняться при упорядоченной
   compaction; ID и физический порядок — нет.
2. Persistent cell membership в каноническом порядке. Изменять только ячейки
   added/removed/moved/woken/geometry-dirty записей и affected hull regions.
3. Полный resting index может существовать, но **каждая ячейка запроса обязана
   проходить прежний точный `want` gate**. Полный y-superset запрещён.
4. Движущиеся фрагменты обновляют frozen start-of-phase boxes каждый тик;
   неподвижные записи переиспользуются. Motion и damage phase не делят устаревший
   snapshot.
5. Hull rider classification обновляется в union old/new affected footprints;
   moving-hull query из ADR-824 сохраняется.
6. Несортированный импорт, snapshot load, большой dirty ratio или сомнительная
   инвалидация вызывают прежний full rebuild. Сам индекс не сохраняется в snapshot.

Важно: это не повтор пробы 424. Там superset менял множество контактов; здесь
persistent storage является лишь способом получить ровно прежние lists. Полный
oracle должен сравнить boxes, cell lists, shapes и ответы запросов, включая wake
вне исходного `asks`.

Ожидаемая область выигрыша — boxes/candidates/cells и связанный wake. Процент до
A/B не заявляется.

### A2. Persistent join membership и heads

Следующий независимый срез после/вместе с профилем A1:

- membership по stable ID и полному key `(pile cell, material, paint, mm)`;
- geometry/version обновляется при переходах resting, позиции, shape/basis/scale,
  material/paint и merge;
- перестраиваются только changed keys и их упорядоченные heads;
- прежний donor order и first viable join остаются обязательными;
- transfer/compaction/import включает консервативную инвалидацию или full rebuild.

В спокойном тике не должно быть полного заполнения `Memo::current` и повторного
`Geometry::of` всей истории. Не возвращаться к отклонённому кэшу ссылок: меняется
владение membership, а не только способ хранения результата.

### A3. Wake и rubble как зависимости, а не новые общие active-list

После A1 использовать тот же набор stable-ID mutations и dirty cells:

- wake получает изменения опор и старые/новые области контактов;
- rubble columns пересчитываются по affected X/Z pages и полным входным доказательствам;
- периодические `support_due` остаются точными scheduled asks, а не глобальной маской;
- сохраняются отдельные снимки начала motion/debris и канонический ordered apply.

Не повторять active-list только для translate, BVH для 62 корпусов или due-mask.

### A4. Active/cold layout — после counters

Как и Box2D, спящие данные не должны засорять горячие массивы. Но canonical order
и snapshot layout не меняются механически:

- hot: ID/slot, pose/velocity, flags, bounds/membership, версии;
- cold: basis, большой chunk payload, редкие material/paint/ledger attributes;
- отдельный упорядоченный список ID сохраняет физическую последовательность;
- запрещены `swap_remove` и порядок, зависящий от hash/пула потоков.

Сначала измерить фактические bytes/capacity; AoS→SoA не является самоцелью.

## 5. Как безопасно подойти к агрегации

Нужно различать три уровня, которые раньше легко смешать:

| Уровень | Что объединяется | Геймплей и golden |
|---|---|---|
| Индексный aggregate | Только broadphase/membership; дочерние фрагменты и контакты прежние | Кандидат на bit-identical, обязателен oracle |
| Packed cold pile | Общее хранение неизменных записей и страниц; при wake извлекаются те же IDs и Fx state | Кандидат на bit-identical; сокращает горячую работу, не обязательно число канонических записей |
| Compound rigid pile | Один body с точными дочерними voxels/materials/ledger; при разрушении локальный split | Может изменить опоры, contact manifolds/order, reclaim и wake; отдельный ADR и новые golden |

Третий уровень ближе всего к остановке роста числа physics objects, но это не
«бесплатная оптимизация». Сохранения массы недостаточно: нужны идентичная занятая
геометрия и пустоты, разрушаемость, материалы, контейнеры и правила извлечения.

Удаление debris по Blast-подобному таймеру, визуальный LOD с потерей коллизий,
увеличение вокселя и средний материал кучи не рассматриваются.

## 6. Обновлённая приоритизация

| Приоритет | Работа | Обоснование из отчёта | Решение/приёмка |
|---|---|---|---|
| P0-A0 | Dirty/touched/bytes counters | Нужна цена полной инвалидации | Раннее/позднее/drain, diagnostic only |
| P0-A1 | Persistent exact `Settled` | 47,193 мс и главный линейный рост | Cell/box/shape oracle, 36 полей, ABBA, 1/4/8, replay |
| P0-A2 | Persistent join membership | Merge keys+groups 41,494 мс плюс полный `current` | First join/order/ledger, cold oracle, transfers fallback |
| P0-A3 | Dependency wake/rubble | Wake 21,053 мс; rubble 26,068 мс в позднем окне | Точные affected areas, motion/debris snapshots |
| P0-B | Packed/aggregate resting piles | Даже идеальный dirty index не уменьшает каноническую историю | Сначала index/packed; compound только отдельным ADR |
| P0-C | Exact bulk explosions | Редкие пики остаются после улучшения steady state | Контакты/энергия/ledger/сроки прежние |
| P0-D | Owned publication + SIM owner | Улучшает frame pacing, не ускоряет сам тик | Lag/debt/tails/bounded bytes |
| P1-A | Gas empty hierarchy/froxels | Независимый render bottleneck | Свежие GPU passes и визуальные golden |
| P1-B | Motion broadphase/nav | O(U²) важен на масштабе, вторичен при 62 | Реальные 100…5000 юнитов, first contact |
| P1-C | Hot/cold и SIMD/layout | Уменьшает цену оставшихся проходов | Только после counters и root A/B |

Порядок не означает ожидание завершения всей серии A перед независимыми C/D/GPU.
Тяжёлые benchmark по-прежнему последовательные. Микросрезы допускаются, но не
должны снова заменять P0.

## 7. Что сознательно не предлагать

- Заменить движок на Jolt/PhysX/Box2D или перевести `Fx` на float.
- Перенести детерминированный solver на GPU: ordered contacts, replay и одинаковые
  1/4/8 workers важнее теоретической пропускной способности.
- Добавить потоки к полным проходам или один глобальный active-list на все фазы.
- Перестроить всё в BVH/OpenVDB/ECS без доказанной модели инвалидации.
- Удалять старые обломки, снижать частоту физики/ИИ, уменьшать урон или разрушение.
- Выдавать отделение SIM owner от кадра за уменьшение SIM mean.
- Обещать процент ускорения по чужим архитектурным статьям.

## 8. Приёмка первого большого прототипа

1. Frozen baseline/source, один причинный change, ранние и поздние ABBA; минимум
   1600-тиковый AI-vs-AI для итерации и 6000/drain для доказательства масштаба.
2. Полный builder сравнивает производные structures и запросы, не только итоговый
   hash. Все 36 нетайминговых полей каждого тика, ledger и snapshots совпадают.
3. Physics 1/4/8 workers, холодный oracle, replay и snapshot в середине боя.
4. Dirty ratio/fallback/bytes публикуются вместе с полным tick; локального phase
   speedup недостаточно.
5. Drain идёт до объяснённого плато по retained records, очередям, flying и памяти.
6. После SIM-приёмки ordinary native 4K проверяет p99, 1% low, темп SIM и debt;
   GPU и publication не смешиваются с выигрышем SIM.

## 9. Конкретный следующий шаг

Сделать A0 и узкий изолированный A1: persistent boxes/membership для `Settled`,
точный `want` gate на каждую запрашиваемую ячейку, stable-ID remap и обязательный
full rebuild fallback. Сначала доказать равенство cell lists и контактов на
направленных тестах, затем запускать AI-vs-AI. Если цена инвалидации съедает
выигрыш, остановиться по данным A0, не разворачивать сразу wake/rubble/join.
