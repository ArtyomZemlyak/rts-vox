# Current UI work — acceptance and actual state

## 668 — Аудит движения, леса (танк), кратеров, объезда и хождения (Opus 5.5) и исправления

Проведён read-only аудит `claude-opus-5-5-high` (`opus-reviewer`) по четырём темам (танки в лесу, движение/коллизии/объезд, кратеры, шагоходы) и внедрены детерминированные P0/P1/P2-исправления без смены версии сейва: для гусеничных корпусов `terrain_changed` переведён на `touches_segment_beyond` (собственная валка больше не сбрасывает маршрут каждый тик и не сбрасывает `blocked_ticks`/`escape`/`hunt`/`coarse`), добавлен проезд промежуточных точек без остановки в `0` (`stopping_speed_to` + `straight_through_deg: 25` в `data/range-navigation.ron`), ограничено ускорение для `sway_of` гусениц и добавлен допуск `terrain_detail` в `uneven`; в `nav::vehicle` запас планировщика вынесен в `Shape::reach` без расширения моста `2*k <= span`, добавлена монотонность уклона `Pose::excess` с гистерезисом `grade_exit_slack_mm: 100` (`unbounded_grade_exit` при `escape_left > 0`), `clamp_tilt` в `phys` гасит только внешнюю радиальную `w`; в `range_motion` убраны отмена заказа при конце старого маршрута (`retargeted_path`), сброс поиска при касательном обходе и преждевременное обнуление `blocked_ticks`, а `apply_walking_terrain` выводит предел уклона из геометрии (`walker_rig.rs` валидирует `support_x/z <= 400`). Отчёт: docs/analysis/668-motion-forest-tank-craters-avoidance-audit-and-fixes-20261119.md.

## 667 — UI-переход к референсной палитре navy/cyan, чистым маркерам и компактным островам

Выполнено визуальное ревью текущего HUD относительно 9 референсов (`out/ui-review-20261117/references-contact.jpg`) и первые P0-улучшения в `panel/theme.rs`, `construction.rs`, `ui_motion.rs`, `panel/minimap.rs`, `panel/hud.rs`, `strategic_icons.rs` и `spatial_ui/chrome.rs`: активные кнопки/фильтры/вспышки выбора `PIXEL` переведены с коричнево-янтарного на холодный `navy/cyan` (`AMBER` сохранён только для прогресс-баров производства и ресурсов); убрана тяжёлая шапка `ТАКТИЧЕСКАЯ КАРТА` (`MiniRoot` `154×154`); облегчён `HeaderIsland`; уменьшены мировые маркеры юнитов (`30×16`, полупрозрачный фон/рамка); уплотнены оптические слоты рецептов в проекции (`142×58`) и оформлен остров `LowerIsland::Commands` с угловыми скобками. Снят нативный 1080p-кадр (`out/round667/stage1-1080/0002.preview.jpg`); пройдены `57/57` `ui_motion + optical_ui + spatial_ui + ui_acceptance + strategic_icons` (`skip 0`), `construction::tests` и пересобран `target/release/blast.exe`.

## 666 — Walker04 на холмах и в лесу: устранение ~1с фризов, кэш IK и стабилизация крена

Устранены фризы до `793–946 мс` при ходьбе `Walker04` по холмистому лесу (`wall_ms: 23 224 → 4 976`, `over50ms: 22 → 0`, побитовое совпадение `hash=0x896cb123e3cadbd1` и `bench_surface_motion` CSV/снапшотов до правки крена): при `crush_cost == 0` `Hunt` больше не срывает `prepared_floor_upper` в `probe_pose_contact`, `Probe::scan` использует `prepared_floor_tile_detail` + пакетный `world.column` на склонах, расширены `PAGE_SLOTS` (`128 → 512`) и `GroundCells` (`256×4` с точечной инвалидацией), добавлен `Probe::pose_crush_flag` для `Motion`, а в `burst.rs` оптимизированы `SettledShape` и `UnitVoxels::any_solid_box`. Также в `data/walker-rig.ron` / `modding::walker_rig` / `range_motion.rs` выровнен крен 15-метрового `Walker04` на холмах (`support_x_cm: 240`, `support_z_cm: 200`, `max_tilt_pct: 99`, `inertia: 600`, `settle_tilt_pct: 15`, `body_sway_pct: 8`), а по ревью `walking-reviewer` (`Claude Sonnet 5.5 (High)`) закрыты баги повреждённой ноги (`shape()` / `ride_now()` -> `84%`), потери тяги на склонах `>14°` (`traction_load`) и предрасчитан `LegRig::retracted` в `gait.rs`. Пересобран финальный `target/release/blast.exe`. Отчёт: docs/analysis/666-walker04-hilly-forest-and-slope-leveling-20261119.md.

## 665 — Покадровые аллокации spatial_ui / construction и 0 предупреждений sim-core + render

В `construction.rs`, `spatial_ui.rs` и `ui_motion.rs` устранены лишние покадровые кучные аллокации и запросы расценки для удалённых дублирующих элементов проекции (`SpatialRecipe::cost`, `heading`, `hint`, клонирование очереди и аллокация `String` до сравнения неизменившихся подписей `Caption::Toggle` / `CatalogToggle` / `Tooltip` / `Name` и `IntensityLabel`); в `construction::tests::fixture` союзный `Tank` вынесен за радиус автоатаки холостого вражеского `Tank` (`[1200, 16, 2800]`), чтобы после ускорения пробития ядра в `662` бой не завершался `Phase::Victory` раньше 200-тикового заказа завода. Устранены все `32` предупреждения компилятора в `sim-core` (`3 → 0`) и `render --bin blast` (`29 → 0`). Прошли `101/101` в `ui-animation + smoke` (`skip 0`) и `41/41` в `spatial_ui + ui_acceptance + construction::tests` (1 явный `--skip` прежней full-world проверки `all_stock_scenarios_offer_only_their_live_allied_builders_recipes`). Пересобран финальный `target/release/blast.exe` с изменениями `662–665` (`sha256=4180210387b634f5…`). Отчёт: docs/analysis/665-ui-spatial-catalog-alloc-and-zero-warnings-20261119.md.

## 664 — Побитово идентичный TileQueueSet в material_flow и параллельный support_sources

Устранён пик `tick 587` (`89.12 → 65.62 мс`, `over_75ms: 1 → 0`) и снижено общее время 1100-тикового ядерного сценария `4096×192×2048` на `−12.84%` (`28983.71 → 25262.30 мс`, `p95 −13.04%`, `p99 −12.98%`) при полном побитовом совпадении `hash=d61f7948e6bd9555` и `removed=30258057`: в `range_granular.rs` / `range_granular_motion.rs` связка `Positions + BTreeSet` и временный `BTreeSet` правок заменены на ленивый послойный битовый набор `TileQueueSet` (`material_flow` сумма `−62.62%`, пик `33.46 → 6.56 мс`), а в `burst.rs` / `nav::vehicle::support` публикация `support_sources_with_bounds` переиспользует `bounds` из `classify`, распараллелена чанками `par_chunks(256)` и пропускает лишнюю сортировку уже упорядоченных `id` (`rubble` сумма `−21.58%`). Прошли `47/47` целевых (включая два скалярных оракул-теста), `138/138` расширенных сьютов (`skip 0`) и холодное восстановление `W1/2/8` против `W4` на `ticks 265..620`. Отчёт: docs/analysis/664-granular-tileset-and-parallel-support-sources-20261119.md.

## 663 — Полная проверка проб подошвы у отвесной стены и устранение 2-тиковой осцилляции Spider

В `crates/sim-core/src/gait.rs` (и каноне `docs/05-units.md`) добавлен предикат `step_sole_climbable` по плоскому максимуму всех 9 проб подошвы (`V3::UP`), исключающий ложный пропуск отвесной стены при диагональном наклоне нормали и раздувание дуги переноса в `sole_clearance_checked` / `swing_contact_checked`; в `retracted_candidates` добавлены ближние точки коридора `(1/4, 0/1)` с проверкой направления голени без выворота, а в `clamp_arriving_reach` — ограничение вертикального подъёма непосаженного контакта в финальной позе корпуса. Все `53/53` юнит-теста `gait` и `113/113` проверок `movement-quick + supports + walking-blasts + architecture + docs` прошли (`skip 0`). На 7 сценах `623` (с холодным восстановлением `W1/4/8` на `41/301`) все 6 обычных сцен (`2, 3, 14, 15, 17, 22`) побайтово равны `657`, а у непроходимой стены `44` (`scene 16`) путь вырос `14956 → 20169 мм` (в окне `440..540`: `1114 → 2703 мм`, `max_still: 61 → 37`), `reversed = 0`, `up = 0`, `max_ik = 0 мм`. Отчёт: docs/analysis/663-gait-sole-climbable-and-wall-retraction-20261119.md.

## 662 — Сквозной пробой ядра, прицеливание по живой части ядра и MotorScratch

Исправлены застревание `aim_point` на пустом квантованном вокселе при косом угле курса (`verified_world_pos` + добивание живого кластера ядра через пролом в `aim_point_from`), выбор бронебойного `Kinetic` у `Tank` в `Auto` против тяжёлых целей и инвариантная к углу проверка единой сквозной 18-связной полости через ядро (`core_bore_pct: 15` и учёт полного `Part::Core` без включения внешней брони и без суммирования раздельных мелких проколов). Все 3 независимых RED-теста `round662` переведены в GREEN (`16/16` focused, `78/78` combat suites). Одновременно перенесён побитово идентичный `MotorScratch` + порядок проверки окна торможения в `range_motion.rs`: ABBA 300-тикового боя сохранил `hash=83e934dbc487b775` при снижении `motion_commit` на `−20.42%` (`terrain_pose −23.70%`, `feet −24.39%`; `50/50` focused + `105/105` movement/supports suites). Обновлён `target/release/blast.exe` для «Полигона» (`sha256=48f2358566abe4f3…`). Отчёт: docs/analysis/662-combat-core-bore-aim-and-motion-scratch-20261118.md.

## 661 — Следующие заказы и настоящее завершение производства

Source `a6e6775d`: два места следующих заказов с количеством повторов и
остатком; в раскрытии отдельно показаны настоящие сборка и оплата.
Прошли 25 focused, 72 expanded и 15 architecture/docs проверок; один прежний
дорогой full-world тест явно пропущен. Настоящий оплаченный Tank создан на
тике 200, очередь продвинулась, выход освобождён к тику 351. Холодный restore
W1/W2 совпал по хешам на всех 240 тиках. Записаны 12 секунд завершения
производства и 11 секунд UI motion; 16/16 проверок ввода, стабильные нижние
якоря, перекрытий 0. Проверены три разрешения и эффекты 100/35/0.
Боевой background A/B на одинаковых входах и тиках записан отдельно.
Обычный FPS, масштаб и общий допуск 627 остаются открытыми.
Dev: `out/ui661/blast-ui661.exe`; release/«Полигон» сохранены.
Отчёт: docs/analysis/661-ui-upcoming-orders-and-real-production-exit-20261118.md.

## 660 — Иерархия production HUD и лёгкие optical chips

Source `64951ca2`:производитель отделён от текущего заказа,disclosure и aggregate
сохранены сверху;optical recipes получили прозрачные угловые chips;inactive
filters спокойнее,selected pixel phosphor янтарный.71focused passed,1явный
старыйstock-worldskip.5финальных native states/3разрешения и3realtime1×ролика
100/35/0:16/16input в каждом,стабильныйminimap/Stop,overlaps0.
Fighting background AB наобщих60..307тиках:108016.309→16.216FPS,
4K16.248→16.251;workload signatures равны,SIM~0.72×,ordinaryFPSgate открыт.
659heatmap/walking/SIM/data сохранены. Devout/ui660/blast-ui660-final.exe;
release/«Полигон» не обновлены дообщего627допуска. Отчёт:
docs/analysis/660-ui-production-hierarchy-and-optical-chips-20261118.md.

## 659 — Тепловая карта крупных отделившихся секций

Source `ab378820`:исправлен подтверждённый RED путь, когда смена diagnostic
ждала SECTION_WORK и оставляла куски с обычной краской. Теперь цвет/материал
переключаются отдельно от формы;разрез обновляется,краска точно восстанавливается.
39focused+15architecture/docs passed,skip0;48секции/разрез/reset/новые секции
и настоящий native laser с803→2210фрагментами проверены через previews.
SIM/data неизменны;devout/round659/blast-round659.exe,release/«Полигон» не обновлены
из-за прежнего общего627gate. Точная пользовательская камера/поза не присваивается.
Отчёт: docs/analysis/659-laser-fragment-heatmap-presentation-20261118.md.

## 658 — Patch22: ceiling memo и crater planes

Source `57051085`:22 перенесён в blast + nuclear tests с исправлением partial
tree-cache X alias (RED→GREEN).31focused/136expanded passed,skip0; отдельные20Python
и реальный timeout smoke закрыли зависание test-fast cleanup.
Nuclear1100:removed30258057/hashd61f7948e6bd9555,все physical counters/21hash/snapshot
byte equal; coldW1 everytick835hash,W2/8 everytickphysics+43hashchecks passed.
Обычный300tick бой/855команд,ABBA и W8checkpoint равны657. Native4K checkpoint
и fighting background1080/4K проверены отдельно от paused кадров; ordinaryFPS/
5000/остатки623/628/627 открыты. Release/«Полигон» не менялись.
Отчёт: docs/analysis/658-antigravity-wave-ceiling-and-crater-planes-20261118.md.

## 657 — wall step и retracted swing без возврата17

Source `aa96a2ce`:21 перебазирован только в gait.rs, все проверки656
сохранены.49gait/59quick/163expanded/89movement/3docs passed,skip0.
7×620 coldW1/4/8:сцены2/3/22/14/15/17 byte equal656;wall16 drop/rise
214/383→93/224мм,Ready2→6,root3147→1542мм,продвижение0→1127мм.
Walker1532rows/4snapshots byte equal. W8battle164→464 restore прошёл.
4native620tick replay/4realtime clips проверены через previews. Боевой workload
иAI изменились, поэтому background1080/4K не объявлен изолированным FPS-pass.
Фото-последовательность623,общаяплавность/масштаб/FPS/627 открыты;
release/«Полигон» не менялись. Отчёт: docs/analysis/657-antigravity-wall-step-and-retracted-swing-20261118.md.

## 656 — indexed gait memo и nav crush/belt

Source `633010d4`:20 перенесён только в gait/nav с независимыми pre20
оракулами.46gait/173nav/56quick/160expanded/86movement/3docs passed, skips0;
4×620 coldW1/4/8,Walker1532rows/4snapshots и боевой W8 checkpoint побайтово
равны655. ABBA:commit9.772→9.522мс,total36.687→36.222мс, но p99 немного
выше; первый отрицательный pair сохранён. Native forest24с и background1080/4K
проверены на том же source.17 не возвращён; wall16/commit tail/5000/ordinaryFPS/
627 остаются конкретной работой. Release/«Полигон» не менялись.
Отчёт: docs/analysis/656-antigravity-indexed-gait-memo-and-nav-crush-belt-20261118.md.

## 655 — verified frontier и рабочий deferred cache

Source`418823c1`:19 перенесён с исправлением hash seen_deferred (0→97.37%hits
на bounded zxy fixture).23focused/53quick/157expanded/83movement passed,skips0.
Квоты/порядок/все нетайминговые счётчики сохранены;4×620cold W1/4/8,Walker1532rows/
4snapshots и W8live checkpoint побайтово равны654. Итого ABBA felling mean
1.679→0.862мс;
разброс total вынесен явно, не выдан за устойчивый общий допуск.
Native forest24с и fighting background1080/4К записаны. Source/proofs/build
финальной версии — out/round655/final; исходный19 оставлен отдельно.
Origin cf7 разрешён как пользовательский play-range -Build; exact historic
source/immutable delivery/live launcher chain не присвоены.17 не возвращён,
623/625/627 и общий FPS-допуск открыты; release/«Полигон» не обновлялись.
Отчёт: docs/analysis/655-antigravity-verified-felling-and-release-origin-20261118.md.

## 654 — Patch18 с исправленными границами, механика652 сохранена

Перенесены cut_feet/sorted motor/lazy relief/stack feet/resting debris.
Исправлены upstream half-open ceiling и AIR-domain; RED2→GREEN5.
Source`4bbcacc0`:53quick/21debris/157expanded/83movement passed, skips0;
четыре623620tick CSV/tapes и Walker1532rows/4snapshots побайтово равны базе.
Cold W1/4/8 и W8live checkpoint прошли. ABBA: feet1.144→0.988мс,
полный тик37.314→37.089мс(-0.6%),p99 практически прежний.
Native forest24с и fighting background1080/4К записаны; обычный FPS/release/
5000 и общий performance-допуск открыты.17 не возвращён,623/628 остатки сохранены.
Release/«Полигон» не обновлялись. Отчёт:
docs/analysis/654-antigravity-cut-feet-motor-and-resting-debris-20261118.md.

## 653 — Patch17 отклонён по управлению, runtime652 сохранён

Свежие четыре620tick прогона подтвердили: старые623 падения621/363мм на
указанных тиках уже закрыты12/650.17 уменьшает другие скачки и даёт4Ready
у стены, но на рампе при forward+turn создаёт30тиков ожидания вместо3,
путь участка5898→1971мм. Пять вариантов проверены отдельно на cold workers1/4/8;
зелёные focused/IK0 не выданы за допуск. Ни один вариант не принят;
возвращены только наши два экспериментальных файла, все прежние изменения
сохранены (source`a968a704`, полный compile-input audit). Диагноз/усиленные
тесты/отрицательные receipts сохранены в out/round653. Новая native/release/
Polygon доставка этим пакетом не выполнялась. Общая вертикальная реакция,
историческая поза и performance-допуск остаются конкретной работой.
Отчёт: docs/analysis/653-patch17-review-and-responsiveness-regression-20261118.md.

## 652 — wave frontier/orders, UI651 сохранён

На source`a968a704` перенесены15/16; BodyMap::range использует безопасную
inclusive границу и bounded binary lookup. Independent legacy relief/spill/tight
body-map/cached-laser state oracles прошли.53quick/29focused/10phys/157suites/
83movement/50weapons/3wave-restore passed, skips0. Четыре300tick GPU-free прогона
сохранили855commands/non-time counters/hash;mean tick38.214→36.865мс. W8 checkpoint164
с активной волной/30moving/15631fragments прошёл ещё300ticks после cold restore.
651/652 загрузили один snapshot, два paused native viewport PNG побайтово равны;
осмотрены только companion previews. Шесть background1080/4K измерений: FPS слегка
выше,1080SIM0.693929→0.693358×; устойчивого nativeSIM выигрыша не показано.
Общий performance/release/625/627 допуск открыт. Live-only13 не возвращён,
root drops623 оставлены Round7. Release/shortcut мной не менялись; внешний
`cf71185e…` по-прежнему без подтверждённого provenance/chain. Отчёт:
docs/analysis/652-antigravity-wave-frontier-and-orders-20261118.md.

## 651 — курсор валки с прежним поведением и idle Layout UI

Полный patch13 отклонён после боевого A/B: live-only spread сокращал фронт,
но раньше выпускал больше фрагментов и удорожал весь бой. Оставлены cursor,
no-room admission и MatCursor без изменения seed/radius/квоты/FIFO. На source
`a5735785`300tick counters/855commands/hash и1532Walker строки/4snapshots
совпали;motion_felling mean3.406→1.935мс,max6.038→3.194мс. Patch14 закрывает
холостые Node записи и сохраняет владение Progress.27focused/46felling/125suites/
83movement/44UI passed; два явных skip перечислены в отчёте. Два native состояния,
317frame UI clip/16input checks и24с физической валки осмотрены через previews;
два1×видео декодированы. Четыре фоновых боя1080/4K — диагностика, не ordinary/
release/625FPS допуск.628front254k,623root drops и625/627 остаются открыты.
Release мной не собирался; обнаружена внешняя замена target/release/blast.exe
на `cf71185e…` в04:54:36, provenance/launcher chain не подтверждены. Development
651 заморожен отдельно. Подробности и done/remaining:
docs/analysis/651-antigravity-felling-cursor-and-ui-idle-20261118.md.

## 650 — wave capture и Spider abyss goal, UI/nav649 сохранены

Round4:11послеreview сохраняет300tick counters/commands/hash;capture mean/call
12,782→7,945мс,max34,621→22,365мс. 12ограничивает будущую abyss goal без anchor/from
иReady;добавлен bounded GoalEnvelope trace. На source390b2196:7capture,5focused,
125suites,83movement passed,skip0. Четыре620tick coldW1/2/4/8 имеют IK0мм;
прежний532мм tick493leg6 закрыт. Scene2/22 полностью прежние;3/16 заново прошли
native replay,18JPEG+9targeted осмотрены,2видео1×decoded. Историческая точная
поза623 иroot vertical drops остаются. Четыре фоновых боя1080/4K задокументированы;
ordinary/releaseFPS,seed coalescence628,625/627 не закрыты. UI647/nav649,physical
foliage/64hull cap сохранены;release/«Полигон»627M прежние. Отчёт:
docs/analysis/650-antigravity-wave-capture-and-abyss-goal-20261118.md.

## 649 — route/corridor/pose fast paths, UI и walking648 сохранены

Round3 patch10 проверен на текущем648: geometry-only predicates, prepared
коридоры от16 tiles без rubble, bounded stack/heap buffers, median5 и Lookup reuse.
Добавлены5 boundary/независимых oracle checks. Source49462b4b:nav171,
suites102,movement80,focused9 passed,skip0.
300tick counters/commands/terminal hash и logical coarse work совпали;4pose
cases сохранили reads/hash. ABBA route4.952→4.202мс/тик.
Четыре фоновых боя1080p/4K задокументированы;ordinary/releaseFPS и623/628/625/627
остаются открыты. UI647,AMBER,walking648,physical foliage/64hull cap сохранены;
release/«Полигон»627M прежние. Отчёт:
docs/analysis/649-antigravity-nav-route-fastpaths-20261118.md.

## 648 — walking memo и продолжение Spider, UI647 сохранён

Antigravity Round2:06v2 уже закрыт647;07 перенесён с исправлением неполного
Sole/lane cache key,08 с физическим arriving/retry/inner-fold и trace,09 с early
empty-skip guard. Pure07:8 gait CSV/trace и300tick battle counters/commands/hash
совпали;средний SIM тик44,974→36,004мс,commit18,641→9,575мс. Final source41519e2a:
194Python,102suites,80movement passed,skip0. Четыре620tick cold W1/2/4/8 и три
полных native replay прошли;27JPEG и targeted previews осмотрены,3видео1×decoded.
Arriving/planted/Ready IK0 во всех4сценах;departing532мм scene3 tick493 leg6
и exact историческая поза623 остаются. Standard terrain drop692→69мм не выдаётся
за общий предел:в жёстких манёврах есть363/621мм переходы. Четыре фоновых боя
1080p/4K задокументированы отдельно;ordinary/release FPS,628/625/627 не закрыты.
UI647,AMBER,physical foliage и64hull cap сохранены;release/«Полигон»627M прежние.
Отчёт:docs/analysis/648-antigravity-round2-walking-memo-and-continuation-20261118.md.

## 647 — контекст каталога, compact queue и4K метки

Antigravity06 перебазирован выборочно: guards projection=None/Scan ужебыли,
order71 закрыт какэквивалент. Filters/Row имеют одного владельца;у боевого
Tank/Spider ипри закрытом compact drawer лишнего каталога нет. Верхняя очередь
268,8px на540p,до360px наdesktop;root неинвалидируется впокое. Дополнительно
исправлена подтверждённая на646 ошибка UiScale2 уметок/полосстройки.
Source7ea0214a:73focused+12architecture+3docs passed;одинявный stock-world
skip. Шестьфинальных nativeсостояний540p/1080p/4K иреальный pointer/motion
clip проверены;overlaps0,низстабилен,видео1×decoded. Старые926кадры невыданы
зафинальный source. SIM646/ходьба645/ui_motion644/AMBER побайтовосохранены.
Финальный development binary заморожен;ordinary/release FPS,628/627/625
глобальные gates остаются открыты. Release/«Полигон»627M не переключены.
Отчёт:docs/analysis/647-ui-post644-context-and-compact-queue-20261118.md.

## 646 — физическая листва и оптимизация Walker04 после645

На sourcea7c73717 безопасно перенесены sparse hull pages, сегментный отсев,
fragment_box4, RLE/touched cache и caller-owned hash scratch. Штатная крона
снова физическая (dust0); скрытый serde default удалён. SOLID_SAMPLES216
отвергнут после реального walking-blasts отказа: прежние64 и gait645 сохранены.
456 исполнений проверок прошли;8старых snapshots побайтово совпали.
Три пары одногоWalker04: trees3095→2508мс,mixed12750→6227мс;1532CSV строки
совпали. Полный24с native проход0–480ticks и9previewJPEG осмотрены, видео1×decoded.
UI644 и AMBER побайтово сохранены. Четыре background боя73units наticks60–252
дают1080p11,331→11,813FPS,4K11,417→12,056FPS,ноSIM0,59–0,60× иfront259k
остаются. Это не ordinary/release/no-regression допуск. Остаток628/627
конкретизирован;release и«Полигон»627M не переключались. Отчёт:
docs/analysis/646-physical-forest-and-snapshot-optimization-20261118.md.

## 645 — walking transfer после644

Текущий compile source95402aa3 включает три walking-правки и вход в
существующий lab replay для полного нативного манёвра. UI644 и AMBER selection
сохранены. Пройдены94 suites,77 movement,9 replay; три native620tick sequences
и27 preview JPEG проверены. Это механический кандидат: exact screenshot623,
departing contact остатки и FPS/no-regression627 остаются явной работой.
Четыре background battle runs не квалифицированы как обычный FPS;
canonical627M не переключён. Отчёт: docs/analysis/645-antigravity-walking-current-20261117.md.

## 644 — Antigravity layout/motion rebase после642

Source32a7f0aa: guarded Node/Text/Image writes,один владелец высоты каталога,
тихая скрытая Scan,dock_height дляDeck. Эффекты стареют до новых событий,
Duration endpoint сохранён; Hover/Unhover начинают age0 при65–100ms.
Pixel selection/focus сноваAMBER по последнему уточнению; базаnavy,
проекции/Opticalcyan. Взаимоисключение recipes/Row,команды иgeometry642 сохранены.
Construction640 иtime/reverse641 не переписывались. Две исходные regressions
были красными; расширенные проверки active/hidden итрёх разрешений зелёные.

Новые68focused+12architecture+3docs прошли на одном source. Три static captures
иnative motion/production/nav среальным выпуском прошли;3+8JPEG осмотрены,
видео1×decoded. Ошибка временного скрытия More найдена native-проверкой,
исправлена,старыйsource1f99 не квалифицирован. Фоновая серия6runs имеет
неустойчивый плохой1080p хвост:FPS/no-regression pass не дан,SIM~0.60–0.64×.
Этот хвост требуетразбора в627; physical foliage638 всёещёнеприемлемдля628.
Одна release build UI-кандидата прошла за643,377с;binary2f0dd222 сохранёнв
out/ui644/blast-ui644.exe. Триactualsmoke прошли,JPEG осмотрены.
Canonical627M не переключался;FPS/no-regression qualification остаётсяоткрытой.
Отчёт:`docs/analysis/644-antigravity-layout-motion-current-20261117.md`.

## 643 — уточнение handoff Antigravity без изменения очереди

Review627M сверено с текущим кодом. Support empty-mode fix уже есть;
Walker/Commander diff по-прежнему отклонён. В639 отсутствует только отдельный
ранний empty-skip guard. Перенос foliage-dust638 не выполняет628: учтённый сток
не сохраняет физическую крону. Решение о его пригодности отозвано; требуется
физическая агрегация в628,схема без скрытого default и stock-приёмка. Кандидат642
остаётся UI-only и не допускается к общей доставке. Новые C1/C3 из review
сверять с текущим UI отдельными focused проверками,не накладывать старый03.
Подробный done/remaining: `docs/analysis/643-antigravity-handoff-current-reconciliation-20261117.md`.
При регистрации движок/runner не менялись,сборки и тесты не запускались.

## 642 — production context controls и cold HUD

Уточнённый handoff Kimi реализован на source df596f4e: реальный chrome::build
создаёт CatalogToggle и четыре pooled Recipe с именами/превью/Choose actions.
Desktop context shelf опционален; полный каталог возвращается в обычную
палитру при закрытии, compact использует один нижний drawer. Исправлены
застрявшая скрытая Row и двойной hover quote. Цены используют общий live island.
Navy/cyan HUD и optical active, локальный amber production, спокойная minimap,
отдельные нижние острова, Systems overflow, чистые previews и скобы выделения.
Emitter/lifecycle320/220,commands/input/SIM и композиция меню сохранены.

66focused+12architecture+3docs+194Python прошли на одном исходнике.
9matched static native кадров и реальные motion/nav/production recordings
проверены; source-bound receipts в out/ui642. 12повторных фоновых боёв
на одинаковых ticks60–269 не показали заметного UI-штрафа,но SIM~0,63×.
Это roster73,не масштабная/foreground/release FPS-приёмка. Обычный FPS-допуск
остаётся открытым по решению пользователя. Один release candidate46018367
собран за687,705с; три native smoke запуска прошли, JPEG осмотрены.
Копия:out/ui642/blast-ui642.exe. «Полигон» остаётся627M до общего допуска.
Отчёт: `docs/analysis/642-native-context-controls-cold-hud-islands-20261117.md`.

## 640 — construction fixture со штатными timed upgrades

Объединённый пакет Kimi перебазирован только в construction/tests.rs.
Отключение timed_upgrades удалено; питание, оплата и T2/T3/T4 проверены через
обычные sim.step, частичный restore и одинаковое продолжение. Более новые
UI assertions сохранены. Текущие 3/3 и локальные 16/16 прошли на одном
исходнике; полный stock-world sweep явно исключён. SIM/баланс/runtime UI
не менялись. Это test-only приёмка, без нового native/FPS/release допуска.
Отчёт: `docs/analysis/640-construction-timed-upgrades-current-review-20261117.md`.

## 635 — узкий контракт animation после627M

Проверка Kimi доведена на текущем коде:новые6focused проверок,компактный
`test-fast --suite ui-animation`,29/29. Найден и исправлен лишний кадр
hover/retrigger на точной границе:f32 age заменён Duration только у эффектов.
Сроки,пул,текст,области клика,production_exit и команды сохранены.
540p100%/4K0% native прошли,26кадров,шестьJPEG осмотрены;по2настоящих выпуска.
Release собран один раз за620,701с;его35% smoke прошёл,кадр просмотрен.
Canonical«Полигон» остаётся627M,его боевые цифры
не являются допуском635. Детали:docs/analysis/635-ui-animation-contract-20261117.md
иout/ui-animation635/ledger.json.

<!--627M-CURRENT-BEGIN-->
## 627M — итог одного неизменного исходника

Доставлен; реальная цепочка CMD→PowerShell→игра проверена12секунд и закрыта.

Compile source: 6fb3df86e104a215c772b7bbb2b5031e3a64b0e826b3003422059260dacc781f.
Expanded904: b7f598d40ba7fc73ace4b191a0ed11ff0174ae1550566f7e391b1241b64172ee.
Release game: 967bdacc50aff659a9f485808edda7e0c73c0ca5eb36ddd263efd5faea989032.
Release CLI: 61ad7bba05b60b1b66ffaea213475c36f7927dec4a339ad4c78843b1b1f30265.

Новый полный test-fast: 548/548,0failed/ignored. Исходные3 forest
прошли и стандартный профиль на том же M; полные тексты тестов сохранены.
Golden600 осталсяc02bcafc47e9b7b8. Все600 release hashes W1/2/4/8
равны development;300save+300cold совпали. Старые G/H/I/J/K/L не используются
как текущие допуски. Отдельные192event hashes W1/2/4/8 совпали.

Механика:17 стадий и12групп аудита; legacy/cold,
промышленность,Скобы,Гребень180SIMсек,полные ядерные/подземные сценарии
и перекрывающиеся разрушения. Функциональный стресс не является20Hz-допуском:
максимальный шаг при четырёх перекрытиях1220.71мс.
НативныйГребень180wallсек:3600ticks,0dropped;постройка остаётся частичной
при нехватке ресурсов. Это проверка отсутствия зависания,не боевойFPS.
Лазерный бой:2400ticks,120SIMсек,W1/2,
финальный hashe638c7c902c62cc6;cold hashfda2a337169918d5.
Расплавлено29237клеток корпусов;liquid peak/end
3976/3364;
поздние попадания в юниты до окна2400.
Это120SIMсек боя,не непрерывный120сек луч;агрегатный sink не подменяет
изолированные проверки испарения и отверстия.
Равновесие сосудов:обычный374ticks,
sealed258ticks,включая120stable ticks;
workers/restore траектории совпали. Подробные численные summaries сохранены.

Native:72dev/890frames и13release/87frames.
Ручной осмотр companion JPEG,реальные UI11s+11s и Spider10s записи;
50контактных/15временных/20дополнительных лесных листов:
В dev74JPEG открыты непосредственно;20неизменных обзоров сверены побитно
с ранее непосредственно просмотренными JPEG. Все runtime-захваты свежие M;
старые runtime-допуски не использованы. В release15JPEG просмотрены напрямую.
Все320лесных кадров охвачены этим
визуальным осмотром;исходнаяпарнаяпозаSpider не воспроизведена.
геометрия/ввод/очереди540p,1080p,4K проверены. Pixel amber HUD,
cyan optical projections,разделённые островки,стабильный низ и верхняя
очередь приняты по актуальным кадрам; tiny text не искажается.
На540p каталог100px вместо180px,подписи14px сохранены;нижняя зона
не превышает40% логической высоты. Отдельные актуальные dev/release
кадры проверяют видимый шарнирТ3 и разделение цены с проекцией.

Фоновый бой:12уникальных прогонов,общие фактические
тики60–488,одинаковые
входы,affinity8логическихCPU,workers2,cap200,без паузы/захвата/кодирования.
Машина:AMD Ryzen 9 9950X3D 16-Core Processor          /NVIDIA GeForce RTX 5090,Vulkan,драйвер610.88.
Roster:73records,45mobile,
28emplacements. Это не5000unit benchmark.

|Фоновый бой old→M|FPS|ТемпSIM|Кадр p50/p95/p99/max,мс|Private/working GiB|
|---|---|---|---|---|
|1080p|21.201→22.776|0.790452→0.803639|52.19/90.41/112.61/155.47→50.09/95.80/111.95/140.76|18.86/15.88→19.01/15.86|
|4K|20.417→21.574|0.791529→0.809278|53.60/91.12/110.95/152.11→51.06/95.96/110.92/141.10|19.14/15.89→19.17/15.94|

|Фоновый бой old→M|Debt max,ticks|Dropped events,ticks|20×wall−ticks|SIM batch p50/p95/p99,мс|
|---|---|---|---|---|
|1080p|19→19|92.92→84.21|113.47→104.58|40.38/73.31/96.98→36.48/79.88/98.79|
|4K|19→19|92.16→80.58|112.73→100.87|40.40/72.11/95.39→36.28/79.13/97.73|

Эффекты0→100,фоновый бой:

- 1920×1080:FPS+2.753%,SIM+0.550%.
- 3840×2160:FPS+1.325%,SIM-0.274%.

Относительный допуск касается продвижения SIM,debt/drop иwall lag.
Он не является ordinaryFPS/абсолютным20Hz/статистическим tail-допуском.
Квантили кадров — среднее двух run quantiles,max/debt/память — худшее
наблюдение;память1Hz private/workingset общего окна,неVRAM. SIM batch
может содержать несколько тиков,его нельзя назвать latency одного тика.

- Backgroundonly;ordinary foregroundFPS intentionally open by user decision
- SIM rate0.803639/0.809278 remains below realtime;20Hz and5000 scale unqualified
- Framep95 increases90.4134to95.7980ms1080,91.1197to95.9559ms4K;SIMbatchp95/p99 increase in bothresolutions;tail latency acceptance OPEN
- 4K effects pointestimateSIM-0.273840percent is retained;overlappingcommit-framebounds are not statisticalconfidenceintervals or proof of equivalence
- Memory increases in some measurements;full results retained in combat-summary,not a memory improvement claim
- Only73rosterrecords45mobile28emplacements at tick1200;this does not qualify128/1000/5000 matrix
- Quantiles are averages oftwo runquantiles;max/debt/memory worstobserved,1Hz memorynotVRAM;SIMbatch may include multipleticks

|Пакет|Done|Remaining|
|---|---|---|
|17 Лазер|Расплав/сквозное отверстие/испарение/материя;длительный реальный бой иcold|Нет внутри функционального контракта|
|18 Метрики/сосуды|Полный охват live/cut/water/debris;полное равновесие иstable tail|Нет внутри функционального контракта|
|19 Т3|Отдельный шарнир;три ракурса/свечение/выстрел/повреждения|Нет внутри функционального контракта|
|20 Старые скриншоты|Все8 групп,legacy saves,рабочие действия,Greben180,текущие native кадры|Нет внутри функционального контракта|
|21 Взрывы|Мощности/полные ядерные иdeep цели/4K/перекрытия/restore|20Hz функциональным стрессом не доказаны|
|22 Стиль|Матрица optical/pixel/amber,единые tokens,композиция меню|Нет внутри функционального контракта|
|23 Иерархия|Верхняя очередь/агрегация,стабильный низ,островки иреальные click areas|Нет внутри функционального контракта|
|24 Проекции|Emitter/лучи/слои/появление/сворачивание,Commander/Spider бездубликата|Нет внутри функционального контракта|
|25 Материалы|Native/pool,локальные эффекты,читаемость иreduced intensity;цена выше|Общий performance pass не выдан|
|26 Motion|Две1x realtime UI записи иактуальные release routes|Нет внутри функционального контракта|
|27 UX|540p/1080p/4K,input/queue audits,текущие визуальные доказательства|OrdinaryFPS открыт по решению пользователя|
|Дополнительная механика|Originalforest3/Commander repair/cadence/turn/codec;TransitionProof workers/cold|Точная исходная парная позаSpider не воспроизведена;ближайшие манёвры проверены|
|Масштаб/лес628|625 — завершённый план масштабнойматрицы|128/1000/5000 матрица не выполнена;архитектурная forest FPS задача628 остаётся в очереди,isolateddust не интегрирован|

Обычный foreground FPS не запускался по решению пользователя. Разрешённый
12секундный функциональный Normal startup не является FPS-измерением.
Доказательства находятся вout/integration627m;engine source frozen,
QA documents имеют отдельные hash receipts. Queue/steer,context limits,
архивный диалог и базыCodex не менялись.
<!--627M-CURRENT-END-->


<!--627L-CURRENT-BEGIN-->
## 627L — функционально проверен, отклонён по цене эффектов

Source `6ea197132b903318c4b5015ebd767b3440bfe4555f71df7ff0e83ee362926403`.
Один release собран за604,228с: `28456a96a0b5f50ed26fb6369bd79182ec3fd9c6b0bf2cef0f6f7040b4745923`.
546/546, исходные forest3/3, все17 стадий/12групп механики и11headless стадий
прошли. Dev72/889frames, release13/87frames;96dev/17release JPEG осмотрены.
Release600 W1/2/4/8 иsave300+cold300 совпали сdev на каждом тике.

Полный12run background A/B, общие фактические тики60–490:
1080p old→L FPS21,983466→22,519749 (+2,439%), SIM0,793701→0,805572 (+1,496%);
4K FPS20,576350→21,422160 (+4,111%), SIM0,797098→0,807278 (+1,277%).
Но effects0→100 SIM−1,126%/−0,693%, frame-marker bounds не перекрываются
в обоих разрешениях. Строгий допуск цены эффектов НЕ пройден, доставки нет.
Все четыре L-бинарника сохранены побитно; результаты не заменены повторными
замерами. Один изменивший камеру/focus запуск отдельно сохранён как
несопоставимый, повторён только этот случай с теми же входами.

Первый dev nuclear-aftermath DeviceLost сохранён, два отдельных повтора
того же бинарника и входа чистые; причина не установлена. Точная исходная
парная поза паука не воспроизведена; все320лесных кадров проверены.
«Полигон» остаётся626. Ordinary FPS открыт по выбору пользователя,
масштаб5000 не проверен,628 не начинался. Новый634-прототип изменяет только
render UI;21/21 узких проверок прошли, боевой dev effects-пилот впереди.
Ledger L: `out/integration627l/ledger.json`; прототип: `out/ui-layout634`.
<!--627L-CURRENT-END-->


<!--627K-CURRENT-BEGIN-->
## 627K — предыдущий кандидат отклонён, доставки нет

K прошёл544/544,полную механику/workers/cold,dev72native/890frames,
release13native/87frames иручнойосмотр99dev/17releaseJPEG.
Единственнаяreleaseсборка`dcf17ab2` заняла671,779s наsource`d41f5987`.
Но новый12runbackgroundA/B вобщихтиках60–490 не прошёл допуск:
1080pFPS−0,922%,SIM+0,839%;4KFPS−1,772%,SIM−0,369%;drop/walllag4Kхуже.
4Kэффекты0→100:FPS−1,541%,SIM−0,485%,frame-markerSIMboundsнеперекрываются.
Эти результаты сохранены,не заменены цифрами G/J и не приняты как готовность.
«Полигон»626 не менялся. Выполняется ограниченныйCPUphase/support/motor
профиль перед исправлением;проверкифизики не удаляются. OrdinaryforegroundFPS
остаётся открыт по выбору пользователя. ТочнаяпарнаяпозаSpiderневоспроизведена;
ближайшиеполные манёвры,включая все320лесныхкадров,проверены.
Актуальныйledger:`out/integration627k/ledger.json`.
<!--627K-CURRENT-END-->

## 632 — компактный UI; историческая проверка K

J отклонён визуально на540p несмотря на зелёную механику. Новый прототип
сократил каталог180→100px,не меняя подписи14px и anchors. Видимая цена
защищена от проекции;скрытая не резервирует место. 101UI+12архитектуры+3docs
прошли;последние540/1080/4K native routes безошибок,2реальных продукта.
Пять просмотренных текущих preview подтвердили свободный выбранный завод
иразделённые блоки. Изменены4productionrender-файла иоднаUIпроверка;
SIM/data/physical thresholds неизменны. Source d41f5987b8c159bcffe0d6ce31c0f0323c31861e5c6d843c3df738d9166e27ca.
Доклад632 иout/compact-ui632/prototype-visual-review.json.
K прошёл визуальные проверки и release, но отклонён боевым A/B. Полигон626.
Функциональный12s Normal startup разрешён;foregroundFPSнепроводится.

## 629–631 — история повторной проверки J

Forest629: исходные3 текста/assertions сохранены. Проверки прошли3/3
в оптимизированном и стандартном профилях на сохранённых источниках.
Текущий J прошёл новый полный прогон и отдельные исходные3forest
в стандартном профиле:3/3,0failed/ignored,40,509s;build102,67s.

Дополнительные Commander случаи прошли13/13 без удаления проверок и
смены физических порогов. H отвергнут за3регрессии; I — за побочное
изменение reference Strider на112-м тике. Golden не подгонялся: ограничение
нового cadence профилем командира вернуло все256prefix hashes G.
J source92eee063,expanded904aa7fc5bc. J прошёл544/544:259механики,105nav,101UI и остальные пакеты, включая
golden600 W1/2/4/8 с неизменнымc02bcafc47e9b7b8,12архитектуры и3документов.
Нативная/боевая/релизная приёмка J ещё не выполнена. «Полигон»626.

Отчёты629–631 в docs/analysis; актуальный процесс/результаты:
out/integration627j/ledger.json. Пакет isolateddust не читали и не
интегрировали; очередь628 не меняли. Обычный FPS-допуск открыт по решению
пользователя. Короткий функциональный запуск штатного окна разрешён,
он будет выполнен в конце; повторного подтверждения не требуется.

## 627 — исторический G-кандидат; допуск отозван при проверке629

Один неизменный G: compile `c4356e47900578dad9652a0694107f6ab4b3dd5b2e985030409aa4e1e2d56776`,
expanded902 `41f71be0b848c2a6d36033f221893e9df7eee0bcb823ea9fd97a9d8b685a0c45`.
903-файловый архив побитно проверен. Все488/488 test-fast проверок и3финальные документальные проверки прошли,
0failed/ignored; golden600 и независимые192tick-hashes W1/2/4/8 равны.
Все17 механических стадий и12 групп аудита прошли наG: legacy/cold restores,
industrial/«Скобы»/«Гребень»180SIMsec W1/2,полные взрывы и перекрытия.
Старая траекторияSpider161states×8ног×W1/2/4/8 и6×620 ближайших условий
без найденного загиба/ReadyIK-остатка;136cold-continuation ticks совпали.
Точная пользовательская парная поза не воспроизведена и не названа исправленной.

Бой2400W1/2=120SIMsec иcold300 совпали:e638c7c902c62cc6/fda2a337169918d5.
29237 клеток корпусов расплавлено,liquid peak3976/end3364;поздние попадания
сохраняются до2400;материя сбалансирована. Полное равновесие сосудов374ticks,
sealed258,затем120stable;workers/cold CSV совпали.

Dev72native/890frames;release13native/87frames;ручной осмотр65dev и11release
лёгких контактных/временных JPEG-листов,отдельные кадры читаемости540p/4K.
Ввод/геометрия540p/1080p/4K прошли наобоих:очередь сверху,0дрейф нижних опор,
0перекрытий выбранного объекта,2реальных изделия. Панелькомандира корректна,
уSpider старыйнижнийкаталог закрыт. Т3 имеет отдельное возвышение ствола;
исторические группы моделей/действий ивзрывы осмотрены. UI11s+11s иSpider10s
realtime записаны,декодированы,просмотрены временными JPEG-листами безускорения.
Native«Гребень»:3600ticks за180wallsec,0dropped ticks,без зависания.

Release собран ОДИН раз после визуального допуска,662,03s;game
`459b01be30905b35b724e465157222321b4f54cf6c0c3db9c28c9ce257d81517`,
CLI `424472c45902affd2de76d1f205ca317dad55e2847e1cef658e02a416142ec93`.
Все600 release hashes W1/2/4/8 равны development;300save+300cold прошли.

12фоновых release-прогонов,общие фактические тики60–490,одни входы,
Ryzen9 9950X3D/RTX5090 Vulkan,8CPU affinity,workers2,cap200.73roster records,
45подвижных/28стационарных в каноническом roster;это не5000unit benchmark.
Fullworld workload сохранён:fragment peak23895→25768,removed840850→854109,
moving peak35→34,searching24→23,live waves14→13. Междуeffects0/100 уG
потактовые workload signatures/selection/roster совпали.

| Фоновый бой | FPS old→G | ТемпSIM old→G | p50/p95/p99/max кадра,мс old→G | Private/working GiB old→G |
|---|---|---|---|---|
|1080p|22.275→22.313|0.797468→0.806283|50.08/89.39/112.06/150.97→50.96/94.45/108.78/141.09|18.87/15.88→18.87/15.85|
|4K|21.025→21.131|0.800763→0.801951|51.65/89.71/111.81/151.02→52.11/95.70/109.38/141.89|19.19/15.90→19.16/15.92|

Это относительная проверка продвижения SIM,не ordinaryFPS/20Hz-квалификация.
Крупная прежняя потеря среднего FPS/SIM не повторилась. Debt max19→19;
dropped ticks в общем окне1080p88,90→83,36,4K86,79→86,20;независимое
отставание1080p109,22→103,31ticks,4K106,99→106,20ticks. Среднийтемп остаётся
около0,80 от20Hz,не realtime. Пропуски учитываются как сумма per-frame events,
дробное представление сохранено. Ни FPS,ни малая очередь не скрывают этот долг.

Хвосты НЕ объявлены исправленными:SIMcorebatch p50/p95/p99
1080p39,77/71,80/94,32→35,47/76,10/95,41ms;
4K40,22/71,03/95,01→35,94/76,04/96,14ms.
p95кадра такжехуже,хотя p99/max лучше. Это наблюдаемое ограничение,
не статистическая эквивалентность и не общий performancepass. Увеличение
числа обломков зафиксировано;само по себе оно не доказывает причину хвостов.
UIeffects0→100:FPS−0,055%1080p/−0,159%4K;SIM+0,198%/−0,072%,frame-marker
границы темпа перекрываются. ЦенаUI неотделимаот разброса этихдвух пар.
Память — private/workingset сэмплы1Hz только общего окна,неVRAM;
квантили кадров — среднее двух run quantiles,max/debt/память — худшие наблюдения.
SIMcore хвосты относятся к батчу тиков вкадре,не к отдельному тику.

OrdinaryFPS намеренно открыт по выборупользователя;foreground FPS не запускался.
Absolute20Hz/5000 иtail-latency qualification не получены.625 — выполненный
запрос наплан масштабнойматрицы,а не результат128/1000/5000замеров.
Stress4взрывов имеет maxstep1,25sW1/1,22sW2,это не20Hz.

Доставка удержана:штатный launcher открывает WindowStyle Normal.
Попытки отдельного неактивного desktop с0x01ff и минимальными0x0083 правами
отклонены Windows AccessDenied5 до запускаигры;desktop не переключался.
Старый реальныйCMD→WindowsPowerShell→626chain проверен;selector нового
каноническогофайла и итоговыйреальный627chain ещё не выполнялись.
Требуется решение оединственном12секундном функциональном запуске обычного
окна дляфинальной цепочки.Это неFPSзамер;его разрешение пока не получено.
«Полигон» остаётся626/`21022afd…`;канонический файл и ярлык не менялись.

Доказательства:`out/integration627g/ledger.json`,`source-coherence.json`,
`combat-summary.json`,`combat-verdict.json`,`visual-review.json`,
`release-visual-review.json`,`release-reference/proof.json`.

Физический пакетF (`a1a7d71f…`, `out/integration627f`) прошёл447/447,
17полных механических стадий, workers/save/replay,120SIMsec боя, поздние
лазерные попадания/жидкость, полное равновесие сосудов. Все8ног Spider на
старом161-state входе и6×620 ближайших сцен без загибов; исходная парная
поза точно не воспроизведена. Но свежий фоновый dev A/B дал-6,37% FPS/SIM
в1080p и-6,28% в4K: F отклонён,72native-сцены и release не запускались.

Фазовая диагностика60–339: motion38,26→40,16мс, terrain_pose20,89→22,81мс,
полный такт61,91→67,85мс; выросла также работа волн/захвата. Дополнительная
выборка1064RIP/248in-game указала на высоты/Source query pages и128bitdivision.
Это диагностическое вмешательство в собственный дочерний процесс, его FPS
исключён из сравнения. Все остановки потоков сняты, сам прогон завершён.

|Итерация|Фактический результат|Статус|
|---|---|---|
|opt1 height-кеши|50проверок,600хешейF равны,но-4,16% противF|Отклонён,source/exe сохранены|
|opt2 полные samples|59проверок,600хешейF равны,+1,07% противF|Недостаточно;sample-кеш затем убран|
|opt3 память8/256 и точные unit-нормали|60проверок,600хешейF равны,+0,63% противF|Недостаточно|
|opt4 курсор immutable query page вProbe|165проверок,600хешейF равны,+2,21% противF|Недостаточно;полные лучи/source/validity сохранены|
|opt5 точная целая арифметика|196/196,все600хешейF равны;фоновая пара+4,47% противF|Недостаточно для компенсации исходной регрессии|
|opt6 порядок доказательств при покое|196/196,600хешейF равны,+3,79% противF;хужеopt5|Отклонён,изменение порядка убрано|
|opt7 endpoint IK|197/197,600хешейF равны,+4,60% противF|Корректен,выигрыш всё ещё недостаточен|
|opt8 поиск контакта|198/198,600хешейF равны;10,917FPS,+4,37% противF;дополнительного выигрыша в этом бою не доказано|Исходная регрессия открыта|
|opt9 порядок моторных кандидатов|198/198,600хешейF равны;10,921FPS,+4,75% противF;заметного выигрыша к8 нет|Порядок возвращён,кандидат сохранён|
|opt10 точные identity-пути|199/199,600хешейF равны;10,952FPS,+4,86% противF|Регрессия ещё открыта|
|opt11 точные дробные степени2|200/200,600хешейF равны;10,972FPS,+5,17% противF|Фазы:motion_commit24,80→23,27мс;остатокwave/capture/frontier|
|opt12 точные frozen AABB|203/203,600hashesF;полныйpitchedT3cold+sizes;10,999FPS,+5,63%F|Относительная потеря ещё открыта|
|opt13 fixed motor height memo|204/204,600hashesF;11,087FPS,+6,10%кF;всёсостояниебояравноF|Нужно прямое old9cb/current|

Trial11 завершён:200checks,все600хешей и workload равныF.
Фазовая пара завершена и не является FPS-приёмкой. Историческая предварительная пара
`out/integration627g/preflight-workflow.json` завершена:четыре old9cb/G
прогона1080p/4K. Gзамороженc4356e47/41f71be0,архив903файловпроверен;
Полная488/headless/dev-native приёмка выполнена;release-пакет впереди. Предыдущие ошибки новых fixtures
сохранены:legacy base без pitch-extensions и неподдерживаемыйTank pitch.
Новые поля/результаты не наследуютF qualification. После стабилизации нужен
новыйfreeze и полный финальный пакет на одном неизменном source, включая
72dev-native/13release-native, видео/ввод, release workers/replay и12боевых
фоновых сравнений. OrdinaryFPS/20Hz/5000 остаются неквалифицированными;
625 — план масштабной матрицы, а не выполненные benchmarks.

Управляющий reference-скрипт послеF-headless упал доCLI из-заWindowscp1252;
отказ сохранён,UTF8 исправлен только вout helper-ах,600CLI затем прошли.
Вopt5 все31Fx прошли,но старыйtest-fast parser не принимал суффикс
`- should panic`;parser исправлен с сохранением точной сверки имён/статусов.
Отказ и старый runner сохранены;новый runner повторно принял31/31 и отклоняет
FAILED/пропущенные/неожиданные имена. Базы,queue/steer иcontext limits не менялись.

Детальные результаты и история: `docs/analysis/627-final-single-source-integration-20261116.md`,
`out/integration627f/ledger.json`. ОригиналыPNG не открывать для общего осмотра;
использовать `scripts/image-preview.py` и companionJPEG. Не повторять уже
пройденные стадии одного source без новой причины.

## Final626 — tasks17–27 consolidated acceptance COMPLETE (2026-11-16)

Explicit done/remaining matrix: analysis/626-consolidated-17-27-acceptance-20261116.md;
machine ledger and receipts:out/final626.233 focused checks passed;fixed stale
laser snapshot baseline decoding and3 incorrect metric-help paragraphs.
Final d0fc3987… has38 targeted checks;unchanged SIM rlib SHA binds full mechanics
evidence.120SIMsec laser battle W1/2+restore,full vessel equilibrium374/258ticks,
all8 historical visual groups,T3 independent barrel hinge and full underground/
nuclear/overlap scenarios accepted. Dev67/314 and release13/87 scenarios/frames;
three-resolution UI real products/input,upper queue,stable bottom,islands and
projections;two11s realtime clips. General images reviewed as companion JPEG.
12 active same-input release battle runs,shared ticks60–491:1080p22.536→22.358FPS,
4K20.997→20.862;SIM0.800099→0.798383 and0.797340→0.796420. Small negative point
estimates retained;relative delivery accepted within endpoint-frame measurement
resolution,not a statistical equivalence proof. Absolute20Hz/5000 unqualified.
Polygon updated to21022afd…;c644 backed up,selector/unchanged shortcut verified.
This closes tasks17–27 only,not the exact Spider623 pose or the scale625 matrix.

## Scale625 — planned after functional fixes (2026-11-16)

Post-functional acceptance protocol:128/1000/5000 mobile units, workers1/2/4/8,
release headless plus active native1080p/4K, frozen world/inputs, ABBA and the
same6000-tick measured window. Persistent edits/debris/routes/fire and actual
completed orders are workload gates; frame/SIM tails, memory, scheduling debt,
dropped ticks and independent wall lag are mandatory.609's64–73 objects do not
qualify5000. Real-time capacity remains unmeasured by this matrix.
Plan only; no benchmarks/builds or background run started, no queue reorder.
Protocol: analysis/625-scale-performance-acceptance-plan-20261116.md.


## Transition624 — shared proof and nonhealthy support (2026-11-16)

Shared coarse/fine/motor TransitionProof, typed first refusal, real source/sole
metadata and bounded span explanation delivered. Driven DropDown agrees with
fine; passive impulse/Disabled falling is explicit and grants no Ready.
Fixed cached Ready on lost limbs and unconditional Pending on unchanged legacy
reprojection; changed contacts are source/sole/reach checked in the same phase.
212 fresh checks passed on final source0d63e9ee… after surface-level review;
actual occupied-probe levels are distinct from ankle positions and asserted.
Four final independent workers1/2/4/8
processes have identical192-hash series for wake/remove/reclaim, including real
voxel damage, impulse and Disabled, warm/cold and mid-recovery restore.
Report: analysis/624-transition-proof-and-nonhealthy-support-20261116.md.
Polygon c644 unchanged; no new combat-FPS or moving-carrier qualification.


## Spider623 — original screenshot investigation (2026-11-16)

Exact two-leg fold remains OPEN / not reproduced; no speculative runtime fix.
24 gameplay configurations ×620 ticks, workers1/2 +restore,30 focused gait
checks and four continuous native clips cover nearest conditions. One unloaded
foot briefly above knee at a4.4m riser is recorded separately, with free-swing
reach residuals; neither is declared the original screenshot reproduction.
Report: analysis/623-spider-original-fold-reproduction-20261116.md.
Engine/model/Polygon unchanged; this does not replace622 battle qualification.

## Walking610 reacceptance622 — 2026-11-16

Current c644e0b4... passes fresh106 mechanics and eight native battle ABBA runs
against preserved607, with no FPS or SIM-rate regression at1080p/4K.
The old R10/102-check loss is not the current result. Existing R19 support/motor
optimizations retained; no engine edits or executable replacement were needed.
Absolute20Hz remains unqualified. Proof: out/walking622/acceptance.json;
report: analysis/622-walking610-current-combat-reacceptance-20261116.md.

## Usability621 — verified and delivered (2026-11-16)

Separate usability audit fixed small4K HUD, projection/actor and notice overlap,
compact laboratory overlap, small menu text, and shared font restoration on return. Three resolutions, real native
pointer routes and production,6 dev/75 frames +6 release/75 frames passed.
Three11s realtime movies are saved.56 UI +12 architecture +3 docs passed.
Release combat ABBA0/100 effects: 1920×1080: -0.91% FPS; 3840×2160: -1.82% FPS. Full timing tails, memory, workload,
input hashes and SIM realtime ratios: out/ui621/battle-final-summary.json.
This is an active-battle diagnostic, not a20Hz performance qualification;
PerformanceQualified remains false. Polygon: `c644e0b4db0d968718b0a10214053f21a0e4fb96431e8a190d1c56943c0c0c31`;
selector verified, shortcut unchanged,620 preserved. Canon: ui-style-system.md.
Report: analysis/621-optical-pixel-usability-20261116.md. No background run implied.

## Native materials619 / motion620 — verified and delivered (2026-11-16)

Shared event-driven motion and procedural finishes are delivered with49 focused
UI,12 architecture and3 docs checks. Seven final development cases/65 frames
and seven actual release cases/65 frames passed. Real production2 products,
cancellation, terminal completion,10 navigation inputs, reduced effects,960 and4K
are verified. Tooltip is a separate island outside scroll clipping; in-flight
frames follow measured layout. Glyph cores match exactly in four full/reduced
regions. Canon: docs/ui-style-system.md. Polygon release:
`8fbeda321911d201b84f519e9edeeae1b09fc24d51b3a37e6a754f6870c08642`. Selector/shortcut verified;618 retained.
No battle-FPS qualification is claimed. Evidence: out/ui620/ and
docs/analysis/620-coordinated-ui-motion-20261116.md. This record does not imply
any live background run.


## User requirements

- The whole UI shares the supplied optical / retro / pixel / holographic style:
  battle HUD, editor/laboratory, main menu, battle room, settings, help, overlays
  and transitions. A single object popup is insufficient.
- All nine UI references were reopened and compared on 2026-11-14. They are
  distinct directions, not interchangeable screenshots. Use the complete set:
  - Precision-framed RTS factory build interface.png: crisp corner frames,
    thumbnail queue, separated hover details; do not restore prices to cells.
  - d560e58a-13b1-434d-b65f-e074d457210d.png: readable production tiers,
    amber progress and clear work order, adapted into the upper production island and fixed lower catalogue.
  - bbc5d763-af62-4d78-a82f-510e83e2559a.png: white/cyan minimal combat HUD,
    quiet small resource rail and transparent unit categories.
  - a5e81164-25bb-4d56-a7bf-9d7f3ddd99db.png: same clean direction, visible
    command hover state; feedback must not obscure the map.
  - 900e0cca-ff58-4b3f-9543-6d6c44111f82.png: lavender optical silhouettes,
    amber secondary lights and sparse world labels.
  - a1935349-193c-4bb1-b7cb-4da7ccbf30f7.png: dot-matrix/raster rendering,
    reserved for previews/readouts instead of making every label noisy.
  - Изображение ChatGPT 10 нояб. 2026 г., 08_52_49.png: menu composition,
    narrow typography, thin rules and semantic optical palette.
  - Изображение ChatGPT 13 нояб. 2026 г., 13_49_56.png: brighter projector,
    ray fan, depth frames and luminous vertices.
  - Изображение ChatGPT 13 нояб. 2026 г., 13_50_03.png: smaller, quieter
    projected production panel. Balance this legibility with the brighter one.
  Files are in C:/Users/artyo/Downloads. Do not reduce the brief to the last two.
- World controls feel projected by the selected building/unit: emitter, thin
  projection rays, layered depth, restrained idle light and an expressive reveal.
  Legible controls must stay stable; do not substitute a flat translucent card.
- Keep world panels compact and intuitive. Show primary commands immediately,
  disclose secondary ones. Match real input areas to the displayed controls.
- Recipe cells show an image and name only. Mass, energy and detailed information
  appear exclusively in a hover popup. Apply this to the legacy catalogue too.
- All UI needs coordinated appearance, hover, selection and menu transitions.
- Preserve local energy, actual recipe quotes, command/queue semantics, input
  blocking, game performance and the cubic world's appearance.

## Spatial projector618 — verified and delivered (2026-11-15)

The selected occupied roof emitter supplies a three-plane perspective rig,
six converging guides, depth struts, luminous vertices and travelling signals.
Rear decoration has slight parallax; ink/hit targets remain stable.320ms
source opening settles to quiet light;220ms fold/exit returns to the emitter.
Fold leaves an independent restore control. Protected placement avoids the
hull, aim centre, production island and lower dock; crowded views retain the
source signal. The hover cache stores the final safe rectangle.

32 focused UI +12 architecture +3 docs checks passed. Seven final dev cases/33
frames and four actual release cases/17 frames accepted via companion JPEGs.
Factory/Spider14-stage render-clock sequences cover real controls and lifecycle.
4K/960×540 and the unchanged menu accepted. Release built once in591.348s;
Polygon selects `d2e86b9bf8fa…`, shortcut unchanged,617 backed up. These are UI
motion checks on paused SIM, not battle FPS. Evidence: `out/ui618/`. Canon: `docs/ui-style-system.md`.
Report: `docs/analysis/618-volumetric-hologram-projector-20261115.md`.

## Spatial hierarchy617 — verified and delivered (2026-11-15)

Dynamic production belongs to a compact upper island, independent of selection.
Its current order, actual progress and following orders remain visible; multiple
producers have an aggregate and paged disclosure. No queue is repeated in the
lower catalogue or object projection. Minimap/categories/permanent orders have
fixed lower anchors. Factory and laboratory use separate islands with actual
hit rectangles; transparent gaps accept world input. The616 style system and
main-menu composition are retained. Canon: `docs/ui-style-system.md`.
Evidence: `out/ui617/`.26 focused UI +12 architecture +3 docs passed.
All12 final dev and5 actual release companion JPEGs accepted, including
960×540 and4K, multiple producers and real construction plans. Shared real
hover/capture price island and displayed actor/generation click bindings are
covered. Release built once in644.997s; Polygon selects `b19e65e6cbc9…`,
shortcut unchanged,616 backed up. Paused UI evidence is not battle FPS.
Report: `docs/analysis/617-spatial-ui-hierarchy-and-production-islands-20261115.md`.

## Shared style system616 — verified and delivered (2026-11-15)

The mode matrix and shared tokens are defined in `docs/ui-style-system.md`.
Battle HUD/static categories use amber pixel chrome; projections retain cyan
emitter/rays/depth; the main menu keeps its composition and quiet CRT layer.
Laboratory, settings and help use light white/cyan service surfaces with dark
ink. Data/previews use local dot-matrix treatment. Lavender identifies secondary
categories, red identifies threat. All nine references were reviewed.

Final development acceptance:35 focused UI +12 architecture +3 docs passed.
11 final current-code native cases and their companion JPEGs are accepted,
including started-battle production/tooltip4K and960×540, Commander and Spider.
Release built once in595.725s with stable source. Five actual release native
cases and their companion JPEGs passed. Polygon now selects cb33ec646099…;
the shortcut is unchanged and the previous c84 release is backed up.
Paused shots produce one still; no CRT temporal film or battle FPS claim.
Report: docs/analysis/616-optical-pixel-ui-system-20261115.md.
Evidence/ledger: `out/ui616/`.

## Full underground/nuclear acceptance615 — verified, performance limitation measured

Exact buried-dome600-tick lifecycles at1x/4x/10x/100x scale real radius,
height and lifted volume, land and drain; workers1/2 and restore agree.
Full stock21-actor nuclear mission kills all20 targets. Full environmental
run destroys all near wood/concrete/steel/forest samples and preserves all
far controls. Four real missiles overlap, kill targets and drain all waves,
world edits, claims, rigid bodies, felling and collapse; workers1/2 and replay
agree. The8m editor-height variant is also exercised with actual matter.

21 Rust checks passed (6 full mechanics,12 architecture,3 docs).
Only test inputs/oracles changed: distant control no longer intersects the
silo; full-volume drain is included in the old timeout; new complete buried
lifecycle regression.6 focused Rust checks passed. Optimized runtime remains
byte-identical to614. Stress is costly: one-world CPU p95/p99=414/1050ms,
peak RAM9.39GB; functional completion does not imply good battle FPS.
Seven dev4K cases and three actual release4K cases passed through JPEG
previews. The complete offline native sequence reaches tick1800, edits0 and
rigid bodies0. Separate30.586s realtime4K stress:9.9066 observed background
FPS, SIM12.25s, ratio0.4005; performance is not qualified. Runtime c84bbac2…
is unchanged; final docs and stock selector passed, shortcut unchanged.
Report: docs/analysis/615-full-underground-and-nuclear-acceptance-20261115.md.

## Historical screenshot acceptance614 — verified and delivered (2026-11-15)

68 Rust checks passed on current code, including full snapshot and600-tick
reference workers1/2/8. Fixed clipped Greben X/Z dimensions, explicit legacy
body/site geometry in snapshot84, missing local power in industrial presets,
and target-camera lookup against the live roster. Stock Greben/Skoba180s
cycles pass workers1/2; four actual legacy saves and10 active-stage restores
pass with balanced matter. No walking/commander/power-plant redesign.
Full Greben construction at180s reaches70.3% from a full commander buffer;
then-current income continues printing. New complete physical geometry costs
more; legacy saved sites retain their geometry and price.17 current-code dev
cases are accepted through JPEG previews. An unpaused180s native run reaches
tick3600, max frame interval0.808s. Release built successfully in681.035s;
11 actual release native cases and their JPEGs passed. Polygon now selects
release c84bbac266d6…; shortcut unchanged and613 backed up. No new battle FPS A/B.
Report: docs/analysis/614-historical-screenshot-fixes-acceptance-20261115.md.

## T3 artillery613 — verified and delivered (2026-11-15)

The current low faceted head and three charge sockets were already present.
The remaining whole-head elevation was replaced by a physical barrel segment
on a fixed trunnion. Muzzle, ballistic launch, heat/hits and drawing share that
barrel frame. Three charge directions were inspected against historical views;
fresh final dev JPEGs confirm36-degree elevation with a level head, actual
barrel damage/molten drop, muzzle flash and an active physical explosion.

Final qualification:53 passed,0 failed/ignored:31 mechanics/full snapshots,
21 render/model/architecture/docs,1 full600-tick reference. Every tick agrees
workers1/2/8; final hash6e1d7e425ee47e19. Full battle restore agrees for100
post-cut ticks. Optional83 retains previous support source metadata so restore
emits exact pending local events instead of a false global cold-catalogue birth.
No voxel scan, motion algorithm or Ready/anchor rewrite was added. Experimental
support_fresh change was rejected and fully removed.

Final dev58eb1410… and actual final releasea51e45246f2b… are accepted.
The final release executed raised damage and active firing/explosion captures;
six fresh JPEGs across five native scenarios were inspected. Build took690.925s with unchanged source.
First candidate0f76997f… was rejected and never delivered; this second build
contains the qualified restore correction. Polygon canonical is now updated;
the stock selector chooses this SHA, original shortcut unchanged.
No new FPS claim. Old saves retain monolithic T3 geometry.
Report: docs/analysis/613-t3-artillery-independent-trunnion-20261115.md.

## Global metrics612 — verified and delivered (2026-11-15)

Terrain speed now uses sparse global active-column publication; original piece,
section and resting-batch draws carry metric colours without the64/local window.
Water shows actual falling speed; missing sideways velocity remains unknown.
Unit meshes follow pose/new damage; full unit caps continue across a frame budget.
Original debris/body geometry is clipped reversibly by the world section plane.
Scope labels and section guidance match this behaviour. Optical UI remains602.
Final source `d16fd5216ba4…` passes96 Rust and15 Python checks. Five native dev
processes passed; six companion JPEGs were inspected, including actual unpaused
live progression35→125 and240/240 blast pieces coloured with pending0.
Full stock vessel levels settle by255 and are confirmed at374 (18.7s SIM),
workers1/2+restore60 CSVs match. Explicit sealed-floor control confirms at258
(12.9s SIM); stock scene, water stepping and tuning are unchanged. Missing fields
and sparse-table overflow remain explicit unknowns. No new battle-FPS claim.
Single release `6ced6850…` passed with unchanged source and actually ran live
metrics35→65; two release JPEGs were inspected. Canonical delivery and the
actual Polygon selector match this SHA; the shortcut is unchanged. Previous611
is preserved in `out/metrics612/before-release.exe`.
See [612](analysis/612-global-metrics-and-full-vessel-settlement-20261115.md)
and `out/metrics612/ledger.json`. Queue/context/archive and prior WIP are preserved.

## Laser/body melt611 — verified and delivered (2026-11-15)

Body melt now retains physical matter as finite liquid drops, with bounded
inelastic flow, cooling/freeze and volume-scaled laser vaporisation. World and
unit melt share phase tuning.115 focused checks pass (0failed/ignored), including
360-tick through-hull/world courses and cold restore. Full600-tick reference
matches every tick at workers1/2/8. A stale support-column cache exposed by this
battle is fixed for real Strider/Spider sole proofs; original feet scheduling
is retained. Two120-s playable AI battles at workers1/2 match final and replay
hashes. Both current native dev JPEGs were inspected; optical UI remains602.
The single release build passed with unchanged source. New release `07e0369f…`
ran an active AI battle; its tick510 hash matches sim-test exactly. Canonical
delivery and the actual Polygon selector match this SHA; the shortcut is
unchanged. No new battle-FPS A/B result is claimed. See
[611 laser report](analysis/611-laser-and-body-melt-20261115.md) and
`out/laser611/ledger.json`. Paused lab captures and headless costs are not battle
FPS. The user's queue, context settings and archived chat remain unchanged.

## Walking recovery610 — verified and delivered (2026-11-15)

Source `db0e5cde2939...` passes104 focused movement/source/lifecycle/gate/forest
checks, failed0/ignored0. Actual incoming-sole Strider continuation reaches the
original goal after~35.08m and93 landings, with Ready IK0 and planted XZ slide0.
All24 checkpoints match mechanically accepted R7 byte for byte. The current dev
native continuation passed JPEG review; UI remains602. Bounded source indexing,
no-op ray culling and rank-preserving support-proof pruning reduce the measured
cost without relaxing physical admission. Final release `a0bc610e810f...` was
built from the same source and exercised in eight active AI battle runs.
Whole30s FPS:1080p 21.77 → 22.74;4K 20.68 → 20.36. Active-window percentiles, workload,
memory, limitations and evidence are in [610](analysis/610-walking-support-recovery-20261115.md).
The canonical release and actual launcher selector match `a0bc610e810f...`; the
desktop shortcut is unchanged. See `out/walking610/delivery19.json`. The small4K
mean regression and unmet real-time target are explicit in the report. No archived chat was started.
Queue order, context settings and existing working-tree work were preserved.

## Battle FPS 609 — measured; candidate held (2026-11-15)

The frozen candidate `0df6bdd3…` retains the optical UI and scopes walking route
cancellation around validated current soles. All four overlapping-blast detours
now finish, but direct incoming-sole Strider continuation still fails; acceptance
is incomplete. A broad height-correction experiment was rejected and restored.
Dev and one isolated release candidate `9a54c0df…` were built from this source.
Four companion JPEGs were inspected; the active AI battle A/B uses separate raw
frame clocks and Windows memory counters in1080p and4K, with repeated runs and a
shared simulation-tick window. It does not use paused UI or one-unit captures.
Performance does not improve and simulation falls behind real time. Detailed
percentiles, roster/world load, memory and provenance are in
[609](analysis/609-active-battle-fps-ab-20261115.md) and `out/battle609`.
The launcher still selects delivered607 `d7f0f684…`; candidate is not delivered.
Remaining: actual underfoot continuation and real-time combat performance.
No background work is implied; queue order and context settings are unchanged.

## Overlapping blasts 608 — checked, acceptance failed (2026-11-14)

Separate full BlastRange walking checks now exist in `--suite walking-blasts`.
Final stable source `8c13e07b…`: 2/5 Rust tests pass. Commander and Strider reach
the detour goal; Spider and Walker04 miss the 900-tick deadline. A direct hit at
the actual incoming Strider sole target earns new Ready after excavation, but
only advances 0.252 m after that landing and fails continuation. Effective engine
and traction remain 1, with no lost/buckled legs; the final four feet are Ready.
All final restore/workers1/2 comparisons pass. The four detour courses pass
67/69 applicable conditions; the two missing conditions are arrival, not replay.
Three section-only conditions are N/A for the other rigs.
Three native runs / six JPEG previews were inspected. Runtime/UI and delivered
607 release are unchanged. These are recorded failures, not a battle-walking pass
or a battle FPS measurement. See [608](analysis/608-walking-during-overlapping-blasts-20261114.md)
and `out/walking608/ledger.json`. Queue order is unchanged.

## Live support 607 — accepted and delivered (2026-11-14)

Current source `cb668662…` adds source ID/content/pose evidence, stable-only small
debris, explicit Moving/Unsupported/Removed recovery and separate Pending/Ready/
Rejected admission. Only Ready votes in the coordinator and window. Local source
dirty pages are separate from nav height invalidation; equal-height replacement
rechecks the sole. Cold restore rechecks saved Ready before XY even without dirty
pages. A failed goal is cached with its forecast-frame; new pose/Recovery/source
event permits a retry. No new per-voxel world scan is used.

Fresh optimized acceptance: 80 SIM + 11 nav + 2 modding, 12 architecture + 3 docs,
27 Python checks, eight flat/terrain hard-turn runs with actual Ready, snapshots
and workers 1/2. Trace audit 40/40: planted slide, all recorded IK, takeoff IK and
zero-ready ticks are zero; Ready minima 1/1/2/4. Eight stairs courses have zero
planted IK and at most one stationary tick after replacing the unsafe unproved
brake fallback with XY/yaw hold and continued foot cadence.

Dev `1c383b3a…` built from the same frozen source. Ten JPEG previews inspected:
Spider/Strider traversal, real carrier Ready and wake, and native continuation of
an old 605 snapshot. Carrier frames use exposed view without a nonexistent patrol.
The single release build passed with unchanged source; delivered executable
`d7f0f684…` passed two live native runs and four reviewed JPEG previews for
Spider/Strider. A fresh read of the real Polygon shortcut and interception of
the actual launcher's selection confirm this same hash. The selected exe was
actually executed in those native runs; the shortcut itself was not modified.
The old 605 release is preserved in `out/live606/blast-605.exe`.
Final delivery audit: 49/49; final documentation checks: 3/3.
See [607](analysis/607-live-support-source-readiness-20261114.md) and
`out/live606/ledger.json`. Other queue tasks retain their order. Moving platforms,
GRF/weak-material capacity, Disabled, explosions and battle FPS remain separate.

## Walking 605 — historical delivered baseline (2026-11-14)

P0/P1 from 593 now have a real radial Spider regression, bounded opt-in reason
trace and immutable world stance anchors. The motor admits XY/yaw against the
same foot coordinator, suspension and bounded physical support constraint used
in execution. A prepared foot outcome is reused after hull fitting; it is not
recomputed into a different lift order. Snapshot format is unchanged.

Final source `d3eb90e5…`: 64 optimized SIM checks passed; 7 nav passes reused
with identical executable and verified proof. Eight fresh flat/terrain hard-turn
runs for four models passed snapshots and workers 1/2: planted slide, planted IK,
take-off IK and window_unready are zero. All eight real Spider rail courses meet
the original 520-tick deadline with actual ankle/sole checks. Four native dev
JPEG previews inspected. One release build passed in 656.592 s with unchanged
inputs: `ffb5180ab7b170aaf0f5a4ad581c535b6ff0d77fbee3faf0a8f09f881047e65a`.
Both native release traversals and their four JPEG previews passed. A fresh
shortcut read and actual launcher selector match this hash; selector launch
was intercepted, while the same exe separately ran both native scenes.
Delivery audit 29/29: out/planted604/delivery-audit.json. Diagnostic games have
finished; do not infer live work from this historical note.
See [605](analysis/605-planted-contact-window-acceptance-20261114.md).

This does not close general SupportRef/Ready, moving carriers, force stability,
damage/impulses, walking during explosions or battle FPS. Spider has a remaining
free incoming-foot IK residual in the hard terrain trace; full swing clearance
is not claimed. Other queue tasks keep their order. UI602's visual acceptance
and complete reference brief below remain applicable.

## Mechanics 603 — narrow gate resolved in current code

- Gate congestion is complete in current source: fresh terrain replanning stays
  eligible while unchecked startup motion remains held. See
  [report 603](analysis/603-narrow-gate-terrain-replan-20261114.md).
- Real ten-tank convoy: 10/10 arrived by tick 540; longest default stop 98 ticks.
  Reverse convoy completed by 664; one-search-per-tick stress by 1952/2000.
- Forty-one related checks passed; final three gate checks and standalone
  authored scenario passed. Per-tick hull/wall clearance and worker replay hold.
- Evidence is optimized headless SIM, not battle FPS or a new render release.
  The later 605 Polygon release above includes this fix; the UI602 build below
  remains the historical UI acceptance artifact.
- Only the gate item is closed here; other queued tasks retain their order.

## Latest acceptance — UI596 completion (602)

- The active UI596 follow-up is complete. See
  [report 602](analysis/602-ui596-final-verification-20261114.md) and
  [current design QA](../design-qa.md); older deliveries below are history.
- The catalogue root has one final visibility owner after the current world
  projection. A visible spider/commander/factory hologram excludes the lower
  catalogue; closing the projection restores only a requested catalogue.
- Shared text/glass/frame/focus tokens now include the legacy settings controls.
  Settings fit content, scroll in short windows and retain native text sizes.
- Twenty focused final-source UI checks passed; the earlier 21-check stage also
  exercised the real factory queue and map-input blocking. An additional full
  debug battle-world settings fixture was stopped incomplete and is not a pass.
- Ten fresh final release frames were inspected through preview: spider,
  actual commander, factory 540, factory 4K with quote tooltip, lab 4K,
  home/room/help/settings and readable compact settings.
- Release: target/release/blast.exe, SHA-256 `59edfcf22490d634cc6a3ca919c2b926b5dfe33e79bf22c52ab27b265e5e2dfc`.
  Compile inputs: `a2af2dbbe926a4c5137d8682c2dfce1d23604d8e177356b573c42f5cb73563a2`. The actual launcher selector matches this exe;
  shortcut source was freshly read, not replaced. Native captures execute it.
- UI captures are paused; they are not battle FPS evidence. Mechanics proofs
  and the user's remaining queue stay separate; no queue/context changes.
- Done: all requested UI596 completion items. Remaining in this UI task: none.

## Implemented before the global redesign

- spatial_ui.rs plus spatial_ui/chrome.rs: pooled world card, 328 logical pixel
  width (410 at 4K), disclosure controls, delayed recipe tooltip, Handjet numeric
  labels, Oswald command labels, corner marks, a scan line and travelling dots.
- Transparent schematic previews reuse the actual model rasterizer; original
  library thumbnails retain their canvas. The changes are render-only.
- DismissPlacement cancels the placement tool without cancelling queued units.
- Most recent release: out/ui593/release-manifest.json. Nine spatial tests and
  one placement regression passed; five native captures completed.

## Earlier iteration 596 — initial native release validation

- All nine UI references reopened; the mapping above preserves their differences.
- Shared native UI material for glass, scanning and panel reveal; eased button
  light and main-menu selection. No additional dependencies/fullscreen pass.
- Projector ray fan, depth outlines and emitter attached to a real hull point.
- Legacy catalogue cells now show name/image only; real quotes are in the delayed
  hover popup. Both catalogue paths share optical model thumbnails.
- Nine spatial control/layout checks passed. First native 4K render completed.
- Clean-dialogue development captures and current check evidence are in
  `out/ui596/continued/`. One release build completed in 587.96 seconds.
  All eight final native captures exited 0 and their JPEG previews were
  inspected. Binary SHA and compiled-input fingerprints are in
  `out/ui596/continued/release-manifest.json`.

## Validation limits and remaining queue

- Final UI views cover HUD at 960×540/4K, commander at 4K, laboratory at 4K,
  home, battle room, help and settings. The commander-first fixture now shows
  the whole commander and the correct panel. Original wrong frames remain
  historical evidence, not validation of that panel.
- OS mouse qualification through computer-use was denied. Bevy interaction
  tests passed; this is not an OS input/accessibility certification.
- Paused captures are UI evidence only. Spider/walker legs, forest slowdown,
  planted/support, movement during explosions and live
  battle FPS remain separate tasks in the user's original queue order.

## Continuation in the clean dialogue

- Done: compact HUD navigation/readout islands; allied matter and aggregate
  power readouts from the publication; matching laboratory frames and model
  thumbnails; battle-room/help/settings migration; occupied-roof projector
  anchor; removal of unsupported arrow glyphs from common labels.
- Done: development 960×540 and 4K iterations found and corrected overlay
  collisions. Secondary battle actions are disclosed; setup keeps a direct
  Start Battle button. The minimap uses the free side beside a world card.
- Done: 19 focused UI checks, including normal launcher entry, page capture
  state, input/pause isolation, resource readout and projector roof geometry.
  Earlier real recipe/queue/input-blocking evidence is preserved separately.
- Done: all nine references rechecked through a lightweight contact sheet;
  development library, home, battle room, help and settings captures reviewed.
- Limitation: computer-use returned `Computer Use was not approved to use blast`.
  No OS mouse input was performed. Menu transitions use Bevy interaction tests;
  native pages are rendered through the normal states via `--menu-page`.
- Done: final release/native qualification, launcher-chain/hash verification,
  and the root `design-qa.md` refresh. All eight final captures and the
  before/after sheet are linked there. No background game run is claimed.
- For future native menu checks use the small range
  `--range out/ui596/continued/menu-fixture.ron` plus `--menu-page home`
  (or `room`, `help`, `settings`). Laboratory captures use that range plus `--laboratory`;
  omitting the range first constructs the kilometre scene unnecessarily.
- Spider-leg evidence supplied by the user in this dialogue:
  `out/codex-migration-20261114/spider-feet.preview.jpg` (original clipboard
  `codex-clipboard-0bbfda20-5814-452e-95ef-cafc3c4bc989.png`). Two lower legs/feet
  fold upwards towards the hull. This is an observed defect, not a diagnosed
  root cause or a completed fix. Handle with the queued spider/walker task
  after the active UI task; preserve the user's queue order.

## Diagnostics on 2026-11-14 (archived-run evidence)

The 2026-11-13 09:41 → 2026-11-14 04:56 run remained active for 19h15m.
The last public command completed at 18:02 Moscow. Same-thread responses_retry
records show HTTP 408 Request Timeout / Request body read timed out. Subsequent
queued work could not become the next turn while that turn remained active.
This establishes the stalled run and timeouts, not the remote server's root cause.
The rollout is 5.9 GB; history_mode is already paginated. Do not migrate/delete it.

An attempted queue→steer change and a project-only 120k auto-compaction limit
were rejected by the user and reverted. The desired mode is queue, with no
additional context limit. Neither change was a valid fix for the stalled run.
Diagnose why the active request stops progressing and fails to release the
queue; preserve normal queue dispatch and all pending messages.

Diagnostic artifacts: out/codex-diagnostics-20261114/. Current provider HTTP and
WebSocket reachability checks passed. Preserve every existing queued message.

Additional evidence: the last request at 2026-11-13 18:02:50 Moscow was
190,196,304 bytes before compression and 137,925,032 bytes after compression.
Earlier requests of similar size received HTTP 408 after about 15 minutes;
the last recorded retry was at 17:47:56. No response completion or subsequent
tool execution was recorded before the morning interruption. This suggests a
request transfer/recovery problem, but does not prove why the final request
stalled. The built-in provider already supports WebSockets and request
compression is already enabled; blindly enabling them is not a new fix.
Do not copy raw HTTP headers, cookies, credentials, or reasoning into reports.
No repair to the app's transport or queue dispatcher has been implemented.

## Locomotion follow-up 597 (2026-11-14)

Done: the in-game four-legged «Шагоход» (Strider) now retains a diagonal support
pair and admits body movement against the next stance before publishing it.
Landing prediction checks both the lane and link reach; duplicate height fitting
is bypassed for the intact supported four-leg admission branch. Details and
limits: docs/analysis/597-strider-support-window-and-height-20261114.md.

Acceptance: 25 checks passed; flat and hill full-SIM runs cover all four walking
models, snapshot/restore and workers 1/2. Strider flat root Y change is at most
1 mm/tick (paired-support repro was 199 mm); hill max is 73 mm, the course is
completed, minimum two planted feet, planted IK error 0 mm. Real sole/height/
progress regressions and tree-contact coverage passed. Native dev turn plus
release stop/descent previews were inspected. Paused captures are not battle FPS.

Release built once in 636.06 s; hash ef95701e3e26967ab0ceb5554725609ea4b58d3fbee22f9418a4642e2c7e0ff9.
Fresh read-only shortcut inspection confirms Полигон → play-range.cmd →
play-range.ps1. The real launcher code selected this hash with process launch
intercepted; the same release exe separately passed both native captures.
After qualification only spaces on two empty source lines were cleaned;
compile-input equivalence is recorded in out/locomotion597/whitespace-cleanup.json.
No active/background game run is claimed. Current fix is delivered.

The original queue is preserved. Spider's folded feet, planted anchor drift,
forest acceptance, gate congestion, walking during explosions and actual battle
FPS remain separate work. This does not close the general 593–595 support design.

## Forest and pebbles delivery 598 (2026-11-14)

Done: user request for at most 10% loss from forest/minor stones, weaker for
massive units, with gradual braking. data/terrain-motion.ron caps one shared
resistance term and scales it by the motor's initial voxel-body mass. Entry
uses 20 ticks; commanded stops, collision, damage and goal braking keep priority.
Tracked minor-ground detail is 50 cm with continuous belt support preserved;
walkers retain their sole-derived 10–20 cm detail. Four healthy legs start a
diagonal cohort together; committed contact endpoints remain exact.

Accepted: 43 SIM checks, 146 nav checks (byte-identical evidence revalidated),
2 configuration checks; ten types × four surface cases plus real BlastRange
mixed/replay across workers 1/2. All 40 surface passes complete, zero blocked
ticks, worst speed 93.097895%, biggest loss 0.345105 percentage points/tick.
Native dev and release captures are unpaused/realtime, not battle FPS.
Details, limits and intermediate failures:
docs/analysis/598-forest-and-pebbles-resistance-20261114.md.

Release built once in 699.11 s, source stable:
cf5da80c58c207ccd8d521935a855f67f514da282b1900b3f629de549dff1233.
Fresh shortcut inspection and actual launcher selection match this binary;
process launch was intercepted in the selector harness, while the same exe
separately ran both release traversals. Release previews:
out/terrain598/release-lightning and release-strider. No game is left running.
Current forest/minor-stone task is delivered; original queue order is intact.
Remaining: spider folded feet, planted/anchor drift, gate congestion, movement
during blasts and actual battle FPS. Existing uncommitted work was preserved.


## Walking follow-up 599 — delivered (2026-11-14)

Implemented: intact grounded multipeds retain world stance contacts; placement
lane fitting is separated from contact IK in SIM and render. Radial motor
admission keeps bounded height fitting; touchdown uses physical contact reach.
61 final checks passed (52 SIM + 7 nav + 2 forest). Four 300-tick Spider/Strider
flat/terrain full-SIM runs passed restore at 45 and navigation/physics workers
1/2: planted slide 0, planted IK error 0, minimum supports 4/2. Native dev
Spider wide and Strider detail previews were inspected.

Release built once in 636.86 s with unchanged compile inputs:
`b0987a85c3ded84bb8da0f6a72b476ae920ebeda1a1369839fd26e85fd4519e4`.
Both unpaused/realtime release captures passed on Vulkan and their previews
were inspected. Fresh shortcut inspection and the actual launcher selector
match this hash; process launch was intercepted only in the selector harness,
while the same exe separately ran both native traversals. Delivery auditor:
25/25 in out/walking599/delivery-checks.json. No game is left running.
Source, rejected intermediate runs and precise limits:
docs/analysis/599-multiped-world-anchors-20261114.md.

The original queue remains intact. This does not close general SupportRef/Ready,
biped or damaged/impulsed anchor behaviour, moving rubble, gate congestion,
walking during explosions or actual battle FPS. The exact old folded pose
from the supplied screenshot has not been freshly reproduced.


## Walking follow-up 600 — delivered (2026-11-14)

Reports 593–595 and 599 were checked against code. The remaining healthy biped
anchor branch is implemented for Commander and Walker 04: immutable world
stance/take-off contacts, side fitting of landing goals, shared turning cadence
and stance width in motor preview, alternatives, height fitting and feet.
61 final checks passed; eight fresh 300-tick full-SIM flat/terrain runs for all
four walking models passed restore and navigation/physics workers 1/2. All
planted slide and planted IK metrics are zero. Biped forward travel and pivot
match their fresh baselines. Both native development Vulkan previews inspected.

One release build completed in 637.64 s with unchanged compile inputs:
`3b851289f3b12a07eae39bf35f68ae6a0f6c0a3ece2e0bfa3500d4e15a3c6af1`.
Both unpaused native Vulkan release traversals and lightweight previews passed.
Fresh shortcut inspection and the actual launcher selector match this hash;
process launch was intercepted only in the selector harness, while the same
exe separately ran both native scenes. Delivery audit: 45/45 in
out/walking600/delivery-checks.json. All diagnostic game runs have finished.
Report: docs/analysis/600-biped-world-anchors-and-shared-preview-20261114.md.
General SupportRef/Ready, moving debris, damaged/impulsed/Disabled support,
gate congestion, walking during explosions and actual battle FPS remain open.
The exact original folded Spider pose has not been freshly reproduced.


## Walking follow-up 601 — delivered (2026-11-14)

600's immutable origin did not guarantee an unchanged executed contact at
liftoff: late lane fitting still moved/rotated the swing. Removed that fitting
for admitted trajectories and added reachable take-off sampling using the
actual terrain-fitted sole, shared with execution. Unreachable departing-root
preview and unconditional early sampling were rejected; details and limits
are in docs/analysis/601-continuous-takeoff-and-swing-20261114.md.

Final source: 54 SIM + 7 nav + 4 forest + 4 Python checks passed. Eight fresh
300-tick final-* full-SIM runs passed restore/workers 1/2: planted slide,
planted IK and take-off IK all 0 mm. Strider's other swing IK errors in the
controlled runs are also zero; Spider terrain has a remaining 47 mm swing
residual. All baseline liftoff counts retained. Final dev Vulkan Spider and
Strider previews inspected. Forest worst speed 93.097895%, biggest loss
0.345105 percentage points/tick; no new sharp forest brake is accepted.

One release build completed in 638.36 s with unchanged source:
`5f30821162a63e12cc3061ecf3617e43d7cdbadb152dd5d549d3455f12ed193b`.
Both unpaused/realtime native Vulkan release traversals and lightweight
previews passed. Fresh shortcut inspection and actual launcher selection
match the hash; launch was intercepted only in the selector harness, while
the same exe separately ran both native scenes. Delivery audit 59/59:
out/walking601/delivery-checks.json. Diagnostic games have finished.
Ledger: out/walking601/ledger.json.
Do not infer a live run from this note. Source/Ready, moving debris,
damage/impulse/Disabled, gate congestion, walking during explosions, battle
FPS and exact reproduction of the old folded Spider pose remain open.

Документальный runner: первыйCargo-wrapper не вернулся после успешной2,98sсборки;
он остановлен доinventory,отказсохранёнвout/integration627g/rejected-docs-cargo-wrapper.json.
Три проверки исполнены заново черезtest-fast --no-build наранее проверенном
хешем/исходником harnessG:3passed/0failed/0ignored. Это не переиспользование
старого результата теста.
