# 642 — реальные context controls, холодный HUD и раздельные острова

Продолжение текущей UI-задачи по уточнённому handoff Kimi и
`ui-reference-mismatch-review-20261117.md`. Это не новая копия очереди.
Просмотрены три предоставленных JPEG contact sheets, включая все девять
референсов. Изменения относятся к native UI; SIM, навигация, физика,
баланс, сохранения и данные не меняются.

## Контракт и реализация

| Область | Текущий контракт |
|---|---|
| Object projection | chrome::build создаёт CatalogToggle и постоянный пул четырёх Recipe с Caption::Name/Visual::Recipe и существующим Choose(slot) |
| Disclosure | Desktop раскрывает отдельный остров; обычная палитра возвращается при закрытии. Команды сохраняют место; width toggle фиксирован |
| Compact | Один нижний recipe drawer; второй projected recipe слой не показывается |
| Верхняя очередь | Единственная; компактный верхний остров без копии у объекта |
| Семантика | Холодные navy/cyan HUD и optical active; amber локально у производства/прогресса; red threat и lavender сохранены |
| Постоянный низ | Minimap слева, категории отдельно, команды справа, отдельная короткая navigation island; между ними world input |
| Вторичные режимы | Лаборатория, редактор, камера, настройки и сценарии доступны через Systems overflow; команды сохранены |
| Превью | Чистая геометрия/силуэт, nearest; без всеобщего warm raster. Native extraction guard предотвращает белый fallback |
| Выделение | Спокойные ограниченные скобы вместо больших кругов у всех юнитов; настоящие range overlays по запросу сохранены |
| Lifecycle и цена | Occupied-roof emitter, 320/220 мс, лучи/слои сохранены; цена только по hover/явному capture preview |

Реальная интеграционная проверка строит chrome, обновляет каталог с настоящими
model previews, запускает production update и проверяет текст, preview handle,
открытие/закрытие, оплатную очередь, блокирование map input и неизменность
авторской раскладки команд. Старое требование отсутствия Recipe заменено
требованием наличия optional controls и отсутствия второй очереди.
Нативный geometry audit дополнительно записывает фактические rectangles toggle
и рецептов и проверяет protected-object overlaps. Native motion route
раскрывает именно каталог и наводит настоящий cursor/focus на рецепт.
Проекция сохраняет существующий ограниченный список первых четырёх рецептов
текущей страницы; полный набор остаётся в обычной палитре после сворачивания
context shelf. Это optional context, а не замена всего factory catalogue.

После уменьшения каталога focused-тест обнаружил пересечение компактной
tooltip-зоны с миникартой. Её резервирование исправлено независимо от высоты
recipe palette; старый assertion не удалён. Canon `docs/ui-style-system.md`
обновлён по новой семантике и больше не требует amber selection/обязательного
нижнего dock для всех рецептов.

## Доказательства и допуски

Базовый текущий executable собран до правок и сохранён как `out/ui642/before.exe`.
Его source: `40aa8dc0e548ad05962d25e97624e1fd7b2f6536f5e6542914712ae0673a2c3c`.
Проверки исходного состава и точные receipts находятся в `out/ui642`.

Статические сравнения используют штатный «Гребень»26 на range-km, одинаковый
sheltered ракурс и zoom4.4, selected actor1, effects100/35/0, viewport
960×540 / 1920×1080 / 3840×2160. JPEG companion создаются штатным image-preview;
оригинальные PNG не изменяются и не открываются для общего осмотра.
Первая exposed-camera попытка показывала хранилище вместо выбранного завода
и исключена из визуального допуска. Промежуточные source не выдаются за final.

Для realtime используются существующие native recording/usability routes.
Обычная запись не приостанавливает SIM; реальный stock Tank строится 10 SIM-секунд.
Видео кодируются по исходным wall timestamps при 1×, без ускорения. Захват,
пауза и UI-сценарии не являются боевым FPS.

Первый production-record отказал до захвата: background требует --profile.
Отказ и лог сохранены, параметры исправлены. Проверенный baseline-record
прошёл реальные ticks0..240 за12 wall-секунд, видео декодировано.

## Final development source и проверки

Compile source:
`df596f4eb841e5391024222ca44c80d9c0b6f4b7d17a3467e276d636f73f916b`.
Development executable:
`3ff949c854feda51e5cc97fab0bee92e03df9adfecd2e66ec7d6804721e81b1d`.

На этом исходнике заново прошли 66/66 focused render checks, 12/12 architecture,
3/3 docs и 194 Python checks. Один полный stock-world inventory явно исключён
через `--skip` и перечислен в receipt; failed/ignored нет. Более ранние зелёные
наборы на промежуточных source не подменяют этот результат.
`source-scope-audit.json` восстанавливает прежние digests только в памяти и
получает исходный baseline hash: compile inputs вне сохранённых UI-файлов
не менялись. На диске исходники не подменяются. Инкрементальная разница с
исходным рабочим деревом сохранена в `out/ui642/implementation.patch`.

Native iteration обнаружила ещё два настоящих дефекта, которые не закрывались
первым зелёным набором. Recipe hover открывал собственную карточку поверх
самого рецепта: теперь проекция использует тот же живой price island, что
обычный каталог. Кроме того, нижняя Row только скрывалась, но не возвращалась
в Flex: теперь update явно устанавливает оба состояния. Реальный builder test
проверяет закрытый/open/закрытый цикл, обе карточки, цены и команду заказа.

В самом native witness исправлены прежние неверные допущения: selected factory
в штатном «Гребне» имеет id1, не id0; фиксированный screen-point для gap после
перекомпоновки попадал в UI или вне полезного terrain. Теперь ограниченный
список настоящих ground points проецируется камерой, отбрасывает занятые hit
rectangles, затем проходит обычный right-click handler. Прямой записи rally
нет. Старые красные input logs сохранены. Проверка hover требует фактической
строки `HOVER recipe`, поэтому один toggle-click не считается проверкой цены.

| Native evidence на final source | Результат |
|---|---|
| `accepted-map-{540,1080,4k}-{100,35,0}` | 9/9 captures; все companion JPEG непосредственно осмотрены |
| `after-motion-540-100-attempt3` | реальные указатель/hover/disclosure, ошибки0, 317 recorded frames |
| `after-motion-1080-35` | реальные указатель/hover/disclosure, ошибки0, 329 frames |
| `after-motion-4k-0` | реальные указатель/hover/disclosure, ошибки0, 101 frames |
| `after-nav-1080-35` | меню/комната/настройки/лаборатория через handlers, ошибки0, 325 frames |
| Production before/after: 540p35%,1080p100%,4K0% | 6 decoded12-second clips; stock10 SIM-second Tank завершает очередь |

Во всех четырёх input sequences максимумы other-object/recipe-object/lab-island
overlaps равны0. В активном HUD минимальный измеренный шрифт13 physical px
на540p/1080p и26 на4K; service/menu проверяются отдельно. Общий минимум
`min_panel_font_physical` при открытом меню включает скрытый HUD и не является
его видимым размером. При close нижние имена/силуэты возвращаются, категории и
команды сохраняют место. Для близкого540p ракурса проекция уступает место
защищённому объекту; доступен один обычный drawer. Overview native sequence
отдельно доказывает существование и работу самой проекции на540p.

Ролики сохраняют wall timestamps и декодированы; отдельные ключевые JPEG
просмотрены, непрерывный ручной просмотр каждого кадра не заявляется.
Это функциональная и визуальная приёмка UI, не FPS-допуск.

## Цена UI в работающем бою

`out/ui642/combat-summary.json`:12 уникальных20-second профилей, по два
наблюдения каждого варианта; порядок baseline100,current0,current100,
current100,current0,baseline100 на1080p и4K. Cached development sim-test,
8logical CPU affinity255,Rayon2,compute/physics/nav workers2,200FPS ceiling,
Vulkan RTX5090. Без паузы,capture/encoding и foreground; runtime подтвердил
background=true и неизменный render context. Все source/binary/input hashes
стабильны. На общих sampled SIM ticks совпали workload signature,selection,
roster между baseline/current и intensities.

Общее окно60–269:209 настоящих тиков. Roster73→73; пики moving33,
searching22,fragment records15671,live waves4,corpse0,pending world edits0
в sampled rows. Во всех запусках есть реальные выстрелы и пространственный
урон. Это небольшой работающий бой, не цель5000. Memory —1Hz private/working
set внутри того же tick-window,не VRAM. Среднее двух run quantiles;
max/debt/memory — худшее наблюдение. Полные прогоны сохранены отдельно.

| Baseline→current100 | FPS | Frame p50/p95/p99/max,ms | SIM real-time | Private/working GiB |
|---|---|---|---|---|
|1080p|12.464→12.664|78.59/116.41/154.03/182.42→78.60/116.57/152.41/180.44|0.6232→0.6332|18.585/15.666→18.575/15.625|
|4K|12.569→12.523|78.08/118.73/157.10/181.71→78.59/117.39/156.20/182.25|0.6284→0.6262|18.722/15.759→18.769/15.805|

| Baseline→current100 | Debt max,ticks | Dropped events,ticks |20×wall−ticks | SIM batch p50/p95/p99,ms |
|---|---|---|---|---|
|1080p|19→19|108.02→102.35|126.38→121.09|53.84/92.24/129.00→53.45/93.25/126.97|
|4K|19→19|104.80→105.63|123.58→124.78|53.67/94.78/131.67→54.02/92.77/130.78|

Effects0→100:1080p12.556→12.664FPS,p95 115.78→116.57ms;
4K12.428→12.523FPS,p95 118.25→117.39ms. Разницы не следует трактовать
как доказанное ускорение от включения эффектов. Короткая повторная серия
не обнаружила заметного штрафа; она не исключает редкие хвосты. SIM batch —
время всех исполненных за render frame тиков,не индивидуальный tick tail.
Темп SIM считается по первым фактическим endpoint markers.

Это background diagnostic текущего UI-пакета. Абсолютный real-time не достигнут
ни baseline,ни кандидатом; обычный foreground FPS не измеряется по явному
решению пользователя. Нельзя переносить сюда старую627M производительность,
выдавать результаты за release A/B или за5000unit admission. Конкретные общие
gates остаются в623 (точная исходная Spider pose ещё не воспроизведена),
628 (forest),625 (scale matrix) и финальной интеграции627;
эта UI-итерация не заменяет их.

## Release и итоговый ledger

После visual signoff выполнена одна release build за687.705с,code0,
source_stable=true. Source совпадает с final development/tests/native/A/B.
Binary:
`4601836705c07ec103e95232d60bcff53b067eb1d271215f2b2ff93dfad469f1`.
Постоянная копия кандидата: `out/ui642/blast-ui642.exe` (запускать из корня
repository с его штатными data/assets). Исходный target executable и копия
проверены по одинаковому SHA256; повторной release build не было.

Три native release smoke captures:540p35%,1080p100%,4K0%,code0;
все companion JPEG непосредственно осмотрены. Это фактический запуск нового
executable,но не полный release battle A/B или SIM/save/workers gate.
Канонический `target/release/blast.exe` остался627M:
`967bdacc50aff659a9f485808edda7e0c73c0ca5eb36ddd263efd5faea989032`.
Launcher/ярлык не переключался: общий real-time gate остаётся незакрытым.

| Работа | Статус |
|---|---|
| Настоящие context controls + builder integration | done |
| Cold palette / minimap / отдельные lower islands / overflow | done |
| Shared hover quote / Row restore / реальные hit rectangles | done |
| Final66+12+3+194 и явный stock-world skip | done |
| Matched native3×3, realtime input/nav, production completion | done |
| Repeated background UI cost on identical tick-window | done,только диагностический допуск |
| Один release candidate +3 actual native smoke | done |
| Canonical delivery / ordinary FPS / масштабная механическая приёмка | remaining вне642:623/625/628/627; ordinary FPS открыт по решению пользователя |

Нет заявленной фоновой работы после завершения. Собственные игровые,
компиляционные и encoder subprocesses завершились по receipts.
Точные final receipts и отдельный ledger: `out/ui642/final-evidence.json`,
`out/ui642/ledger.json`. Документы описывают принятый UI-кандидат и фактические
общие ограничения,а не готовность всего проекта.
