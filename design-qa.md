# UI — единая optical / pixel система и журнал приёмки

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

## 645 — механический кандидат, UI644 сохранён

Перенесены три walking-исправления после review; потолок стоп учитывает
реальный крен, fixtures кроны и ступени действительно воспроизводят дефекты.
Source95402aa3: 94/94 suites,77/77 movement,9/9 replay, cold restore W1/2/4/8
с hashes каждого тика. Три полных native манёвра по620ticks совпали с replay;
27 лёгких JPEG осмотрены, три38с видео1× декодированы. Amber selection и
острова UI644 не менялись. Исходная двухногая поза623 не закрыта; departing
остатки имеют конкретные tick/leg/q. Фоновое боевое A/B не даёт FPS-допуска:
нагрузки разошлись, у baseline есть почти10с SIM wall выброс. Release этой
промежуточной итерации не собирался; «Полигон» остаётся627M.
Отчёт: docs/analysis/645-antigravity-walking-current-20261117.md.

## 644 — layout/motion исправления и янтарный Pixel selection

Antigravity03 перебазирован выборочно;640/641/642 сохранены. Исправлены
холостые Node/Text/Image записи,сброс responsive высоты,движение скрытой Scan
и пропуск первого кадра коротких эффектов. Две исходные regressions дали0/2,
после исправления прошли; проверка тихих components охватывает540p/1080p/4K,
active catalogue иnative builder. More/fold восстановлены после найденной
native ошибки fallback;catalog shelf закрывается при потере выбора.
Pixel selection/focusAMBER наnavy,Opticalcyan; semantic matrix обновлена.

На source32a7:68focused+12architecture+3docs passed,failed/ignored0,
stock-world sweep явно исключён. Static3 иrealtime motion/production/nav
прошли;3+8JPEG просмотрены,3videos decoded. Цены/команды/верхняя очередь/
один drawer иclick-through gaps сохранены,protected overlaps0.
Шесть фоновых боёв наобщихticks60–267:1080after p95 142.34/112.38ms,
before115.50/112.56ms;4Kafter114.88,before113.30. Первыйплохойхвост
не устранёндоказанно,поэтому FPS/no-regression pass не дан,требуетсяразбор627.
SIM~0.60–0.64×,roster73,не5000. Одинrelease-кандидат собранза643,377с,
binary2f0dd222,out/ui644/blast-ui644.exe. Триactualsmoke прошли,JPEG осмотрены;
«Полигон» остаётся627M. Отчёт:docs/analysis/644-antigravity-layout-motion-current-20261117.md.

## 642 — native context controls и холодные острова HUD

Настоящий chrome builder создаёт toggle и четыре pooled context recipes;
проверяются имена, previews, live hover quote, очередь и команды. Исправлен
возврат нижней Row после закрытия проекции. Полный каталог сохраняется в
обычной палитре; compact использует один drawer. HUD/minimap/optical active
переведены на navy/cyan, amber оставлен у production/progress. Постоянный низ
разделён, debug вынесен в Systems, большие общие круги убраны.

На final source df596f4e:66/66focused,12/12architecture,3/3docs,194Python.
9matched native кадров960×540/1080p/4K,effects100/35/0 непосредственно
осмотрены через companion JPEG. Реальный pointer/hover/disclosure проверен
на всех трёх разрешениях; native nav включает меню/комнату/settings/lab.
Ошибок ввода и protected-object overlaps нет. 6production clips before/after
покрывают настоящий stock Tank до выпуска; видео1×,decoded. Это не FPS.

12чередующихся фоновых боёв,общие ticks60–269,workers2,8CPU,cap200:
1080p12,464→12,664FPS;4K12,569→12,523. Заметной UI-регрессии в этой
диагностике не обнаружено; эффекты0→100% не дали измеримого штрафа.
Но SIM лишь0,62–0,63×real-time,roster73. Это не ordinary FPS,не release
performance и не5000unit admission. Полные p50/p95/p99/max,memory,debt,
workload и ограничения:docs/analysis/642-native-context-controls-cold-hud-islands-20261117.md.

Один release candidate собран после visual signoff за687,705с; три запуска
540p35%/1080p100%/4K0% прошли, companion JPEG осмотрены.
Binary4601836705c07ec103e95232d60bcff53b067eb1d271215f2b2ff93dfad469f1
сохранён отдельно в out/ui642/blast-ui642.exe. Canonical«Полигон» остаётся627M.
Source scope audit подтверждает неизменность SIM/nav/physics/data.

## 635 — focused animation contract

29/29focused и27/27Python-защит прошли. Исправлен только f32-хвост срока
декоративных эффектов:Duration гарантирует точный конец и корректный retrigger.
Реальный production_exit проверен при100/35/0%,один клик — один заказ.
Два native сценария540p/4K,26кадров,шестьJPEG осмотрены;по2реальных выпуска,
0перекрытий объекта. Release собран один раз за620,701с,его запуск с35%
прошёл,540p preview осмотрен. Боевой FPS635 не измерялся;
canonical«Полигон» пока627M. Отчёт:docs/analysis/635-ui-animation-contract-20261117.md.

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

## Приёмка626 — завершена, 2026-11-16

Отдельная таблица заданий17–27: [отчёт626](docs/analysis/626-consolidated-17-27-acceptance-20261116.md).
233 узкие проверки; после3 исправленных подсказок метрик — целевые38 проверок
на финальном d0fc3987…; SIM libraries неизменны, полные функциональные прогоны
наследуются по SHA.120SIMсек боя W1/2+restore,полное равновесие374/258ticks,
промышленные/ядерные сценарии,все8 исторических групп и отдельный шарнир Т3 приняты.
Dev67/314 и release13/87 сцен/кадров просмотрены через companion JPEG.
960×540/1080p/4K:верхняя очередь,стабильный низ,островки,проекции,ввод,два
реальных продукта;две realtime11с записи motion/nav сохранены вout/final626.
12 активных боевых release-прогонов на одинаковых входах,ticks60–491:
1080p22.536→22.358FPS,4K20.997→20.862;SIM0.800099→0.798383 и0.797340→0.796420.
Отрицательные оценки не скрыты;сравнительный допуск учитывает границы записи
крайних кадров и не доказывает точное равенство. Абсолютные20Hz/5000 не приняты,
PerformanceQualified=false. «Полигон» обновлён на21022afd…;старый c644 сохранён,
selector и неизменность Desktop-ярлыка проверены. Полная done/remaining таблица
и показатели p50/p95/p99/max,памяти,SIM/debt находятся в отчёте626.

## UI621 — отдельная приёмка удобства, 2026-11-16

Исправлены мелкий HUD4K, перекрытие соседнего actor проекцией, сообщения поверх
проекции, наложения лаборатории960×540 и мелкие подписи меню/возврат общего шрифта. Проверены три
разрешения, стабильные нижние anchors, верхняя очередь, реальные отмена/два
продукта, fan/gap/deck/menu input и tab/category/scroll лаборатории.
56 UI +12 architecture +3 docs; dev6/75 и release6/75 ключевых кадров.
Три realtime11s MP4, без ускорения; это не боевой FPS.
Восемь release боевых ABBA: 1920×1080: -0.91% FPS; 3840×2160: -1.82% FPS. SIM rate и память приведены отдельно;
PerformanceQualified=false, профиль не выдаётся за гарантированный20Hz.
«Полигон»: `c644e0b4db0d968718b0a10214053f21a0e4fb96431e8a190d1c56943c0c0c31`. Ярлык проверен и неизменён,620 сохранён.
[Отчёт621](docs/analysis/621-optical-pixel-usability-20261116.md), доказательства out/ui621.

## UI619–620 — материалы и согласованное движение, 2026-11-16

Единый цикл панелей, hover90/60мс, selection/tabs120мс, раскрытие180мс,
панели240/180мс, tooltip от источника и переходы меню/комнаты/настроек/лаборатории.
Текст и области клика остаются на конечных местах; команды доступны сразу.
Прогресс80мс следует реальным значениям. Отмена продвигает очередь;
завершение требует нового ID продукта, включая последний заказ исчезнувшей линии.
Одновременные продукты получают разные направления к миру.

Pixel phosphor, dot-matrix, optical и локальный событийный CRT; янтарные
пиксельные превью и глифы. UI effects100/35/0 сохраняет данные и команды;
35 убирает sync/split. Пул512 видимых/закрывающихся элементов,16 событий,
192 stroke-узла и96 материалов. Tooltip вынесен из scroll clip; открывающиеся
рамки следуют измеренному layout. Новые превью имеют прозрачный кадр extraction.

Принято49 UI +12 architecture +3 docs,0 failed/ignored.
Development:7 сцен/65 кадров; release:7 сцен/65 кадров.
Реальный SIM выпустил2 танка; проверены отмена, последние заказы,10 действий
навигации, тихое состояние,960×540 и4К. Полный и сниженный режимы сохранили
точные позиции glyph cores в4 областях.

«Полигон» обновлён: `8fbeda321911d201b84f519e9edeeae1b09fc24d51b3a37e6a754f6870c08642`. Selector проверен,
ярлык неизменён, предыдущий618 binary сохранён. Это UI acceptance.
[Отчёт620](docs/analysis/620-coordinated-ui-motion-20261116.md),
[619](docs/analysis/619-native-optical-pixel-materials-20261116.md),
[release-обзор](out/ui620/overview.preview.jpg).


## UI618 — объёмная проекция, 2026-11-15

- [x] Реальный roof emitter;6 сходящихся направляющих,3 перспективных
  плоскости, рёбра глубины, parallax заднего декора и светящиеся вершины.
- [x] Локальный отклик источника и бегущие импульсы; эффектный выбор и
  тихое чтение, неподвижные текст/реальные кнопки.
- [x] Раскрытие от emitter320мс; fold/выход220мс, отдельное восстановление,
  закрытие старой проекции перед сменой actor. Меню использует отдельный wipe.
- [x] Защита корпуса/прицела/очереди/dock, финальный hover-кэш и43 элемента
  декора без блокировки ввода. Roof проверен у6 типов.
- [x]47 проверок прошли;7 финальных dev-сцен/33 кадра, две14-стадийные
  последовательности с настоящими контролами,960×540 и4К.
- [x] Release591.348с;4 release-сцены/17 кадров приняты. «Полигон»
  `d2e86b9bf8fa…`, selector проверен, ярлык неизменён,617 сохранён.

[Отчёт618](docs/analysis/618-volumetric-hologram-projector-20261115.md),
[обзор](out/ui618/overview.jpg). Это движение UI при paused SIM, не боевой FPS.


## UI617 — пространственная иерархия, 2026-11-15

- [x] Компактная глобальная очередь сверху: настоящий текущий заказ/прогресс,
  следующие элементы, агрегат нескольких производств и раскрытие со страницами.
- [x] Нижняя копия очереди и каталог в проекции удалены; каталог один.
- [x] Миникарта, категории и постоянные команды имеют фиксированные якоря.
- [x] Завод и лаборатория разделены на островки; пустоты пропускают мировой
  ввод, видимые контролы блокируют его своими реальными прямоугольниками.
- [x] Общая hover/capture-подсказка использует живую цену и питание.
- [x]26 focused +12 architecture +3 docs:41 passed;12 финальных dev и5
  настоящих release native-сцен через JPEG,960×540 и4К.
- [x] Release644.997с; «Полигон» `b19e65e6cbc9…`, selector проверен, ярлык
  неизменён,616 сохранён. Главное меню и стиль616 сохранены.

[Отчёт617](docs/analysis/617-spatial-ui-hierarchy-and-production-islands-20261115.md),
[обзор кадров](out/ui617/overview.jpg). Кадры на паузе не подтверждают боевой FPS.


## UI616 — optical + pixel retro, 2026-11-15

Матрица показана до реализации: [режимы и токены](docs/ui-style-system.md).
Боевой HUD/статичный каталог — pixel и янтарь; проекции — cyan и глубина;
меню — прежняя композиция и тихий CRT; лаборатория/настройки/справка —
светлый white/cyan с тёмными подписями. Dot-matrix локален для данных/превью,
lavender — вторичные уровни, red — угроза. Все девять референсов учтены.

- [x] Общие роли шрифтов, сетка4/8, линии1/2, углы и времена90/140/220/320мс.
- [x] Контраст мелких подписей HUD и светлых иконок/превью лаборатории исправлен.
- [x] 35 focused UI +12 architecture +3 docs:50 passed,0 failed/ignored.
- [x] 11 итоговых native dev-сцен через companion JPEG;4К и960×540.
- [x] Настоящий командир, паук без двойного каталога; раскрытое производство,
  реальная очередь/прогресс и отдельный tooltip цены в запущенном battle mode.
- [x] Release595.725с,5 настоящих release native-кадров приняты через JPEG.
- [x] «Полигон» обновлён до `cb33ec646099…`; штатный selector и неизменность
  ярлыка проверены, предыдущий `c84bbac2…` сохранён. Код SIM не менялся.

Ограничение: paused `--shots` выдаёт одиночный кадр. Временная серия CRT
не получена; still-кадры не подтверждают динамику перехода или FPS.

Отчёт: [616](docs/analysis/616-optical-pixel-ui-system-20261115.md).
Кадры на паузе — визуальная приёмка, не доказательство боевого FPS.


Итерация615: полные подземные и ядерные входы проверены с материей,
целями, посадкой, workers1/2 и restore.21 проверка прошла (6 SIM,
12 архитектура,3 docs);
в production-механике изменений нет. В4К принимаются физические пласты,
развитие облака и настоящий кратер в8-метровом грунте. Многоволновая
нагрузка функционально завершается, но CPU и RAM дороги; за хороший
боевой FPS это не выдаётся.7 dev и3 release4К-сценария приняты через JPEG.
Полная native-последовательность дошла до1800, queue0/bodies0. Отдельный
realtime4К stress:9.9066 FPS, SIM/wall0.4005; хорошая производительность
не подтверждена. Runtime c84bbac2… прежний; selector и неизменность
ярлыка проверены. См.
[615](docs/analysis/615-full-underground-and-nuclear-acceptance-20261115.md).

Итерация614:68 Rust проверок прошли. Исправлены обрезка складских модулей
«Гребня», отсутствие питания в готовых промышленных сценариях и камера
для выпущенных юнитов. Оболочка84 сохраняет вариант корпуса/стройплощадки
без смены оплаченной геометрии. Полные180с SIM на workers1/2 и старые
сохранения проверены;10 активных стадий продолжаются точно после загрузки.
17 финальных dev-сценариев приняты через JPEG preview.180с native без паузы:
тики0→3600, максимальный межкадровый интервал0.808с. Полная стройка
«Гребня» на180с достигает70.3% и продолжает печать на текущем доходе.
Release собран за681.035с;11 реальных release-сценариев и их JPEG приняты.
«Полигон» обновлён до `c84bbac266d6…`; штатный selector проверен, ярлык
не изменён,613 сохранён. Нового замера боевого FPS нет. См.
[614](docs/analysis/614-historical-screenshot-fixes-acceptance-20261115.md).

Итерация613: ствол Т3 на отдельной физической цапфе; голова и три камеры
получают только азимут.53/53 проверки прошли:31 механика/full snapshots,
21 render/model/architecture/docs,1 общий600-tick эталон. Каждый тик совпал
workers1/2/8, итог6e1d7e425ee47e19;100 тиков после среза боя также совпали.
Три камеры сверены с историческими ракурсами. Свежие final dev JPEG проверены:
36° ствол при горизонтальной голове, повреждение/расплав, вылет и настоящий
взрыв. Исправление83 сохраняет metadata каталога опор без обхода кубиков;
алгоритм шага, Ready и anchors не переписываются. Первая release-сборка
отклонена; финальная после исправления restore принята и доставлена.
Release `a51e45246f2b…`, сборка690.925с; реальный выстрел/взрыв и
повреждённый поднятый ствол проверены в финальных JPEG. «Полигон» обновлён,
штатный selector выбирает этот SHA, исходный ярлык не изменён.
Нового замера FPS нет. См.
[613](docs/analysis/613-t3-artillery-independent-trunnion-20261115.md).

Итерация612: общий охват метрик земли/юнитов/обломков, штатные batches,
скорость падения воды и обратимый мировой разрез проверены.96 Rust и15 Python
прошли, failed0/ignored0. Шесть финальных JPEG просмотрены; живой native-прогон
действительно шёл без паузы35→125, после взрыва окрашены240/240 кусков.
Полный замер сосудов: штатный18.7с SIM с подтверждением120 тиками,
контроль с непроницаемым дном12.9с SIM; workers1/2+restore совпадают.
Release `6ced6850…` успешно собран за651.942с и реально исполнил живой опыт
35→65; два release JPEG проверены. «Полигон» обновлён, штатный selector выбирает
этот executable; ярлык не менялся, прежний611 сохранён. Нового FPS A/B нет.
См. [612](docs/analysis/612-global-metrics-and-full-vessel-settlement-20261115.md).

Итерация611: расплав корпуса стал физической жидкостью с сохранением материи,
охлаждением/застыванием и объёмным испарением от лазера.115 проверок прошли;
эталон600 сравнен по каждому тику workers1/2/8, два полноценных120с AI-боя
workers1/2 совпали по final/replay. Два новых native dev-кадра мира/корпуса
просмотрены через companion JPEG; UI остаётся602. Кадры на паузе — только
визуальная приёмка, без заявления FPS. Release `07e0369f…` успешно собран
и реально исполнил AI-бой; хеш тика510 совпал с sim-test. «Полигон» обновлён,
выбор canonical exe проверен штатным selector; ярлык не изменён. См.
[611](docs/analysis/611-laser-and-body-melt-20261115.md) и `out/laser611/visual-review.json`.

Итерация610:104/104 профильных механических проверок прошли без ignored;
24 сохранения побайтно совпали с принятой механической версией. Шагоход завершает
приказ после реального попадания под стопу, Ready IK и planted XZ slide равны0.
Текущий native-прогон продолжения хода и JPEG проверены; UI остаётся602.
Восемь release-прогонов активного AI-боя завершены: полный30с FPS1080p
21.77 → 22.74,4K 20.68 → 20.36. Общий активный интервал, p50/p95/p99, нагрузка и память
приведены в [610](docs/analysis/610-walking-support-recovery-20261115.md).
Release `a0bc610e810f...` установлен для «Полигона»; штатный selector проверен,
исходный ярлык не изменён. Архитектура/документы15/15. Средний FPS4К немного
снизился, p99 улучшился; симуляция пока отстаёт от реального времени.

Проверка 609: native dev-превью паука и шагохода просмотрены; optical UI не
менялся. Восемь активных release-запусков боя в1080p/4К завершены; прироста FPS
нет, симуляция отстаёт от реального времени. Приёмка механики:101 проверка прошла,
одна упала (продолжение после попадания под будущую стопу), один старый ручной
бенчмарк ignored. Новый release — только отдельный candidate `9a54c0df…`.
**«Полигон» не обновлён**, selector по-прежнему выбирает `d7f0f684…`.
См. [609](docs/analysis/609-active-battle-fps-ab-20261115.md) и `out/battle609/ledger.json`.


Статус: активная UI-итерация завершена. Финальные кадры сняты после
последних правок из одного нового release и просмотрены через JPEG preview.
Старые результаты и механические отчёты сохранены ниже как история.

Сквозная механическая проверка 608 при перекрывающихся взрывах — **2/5**:
Spider/Walker04 не завершают обход за 900 тиков; Strider при попадании в цель
стопы получает Ready, но почти не продолжает движение. Restore/workers1/2 совпадают.
Три native-запуска и шесть JPEG просмотрены; код UI и поставленный release 607
не менялись. Это фиксация отказов, не новая успешная приёмка или замер FPS.
См. [608](docs/analysis/608-walking-during-overlapping-blasts-20261114.md).

Механический пакет 607: 80 SIM + 11 nav + 2 modding, 15 инвариантов/документов,
27 Python и восемь свежих hard-turn replay пройдены на `cb668662…`.
Аудит трасс 40/40: Source/Ready доступны, slip и IK нулевые; на лестницах ожидание
не более одного тика. Десять dev JPEG-превью проверены, включая реальные обломки,
wake и загрузку старого 605 snapshot. Единственная release-сборка завершилась
на том же source. Четыре свежих release-превью Spider/Strider просмотрены;
два живых запуска успешны. Свежая проверка настоящего launcher выбирает
`d7f0f684873cc3fececf4ee6009369a000811abecd1b41db6219050cc94d28a5`.
Проверка выбора перехватывает Start-Process; этот же exe реально исполнен
в native-запусках. Ярлык не изменялся. Боевой FPS этим не измерялся.
Финальная сверка доставки — 49/49, документы — 3/3.
См. [607](docs/analysis/607-live-support-source-readiness-20261114.md).

Исторический executable после механической доставки 605 (сохранён в backup):
`ffb5180ab7b170aaf0f5a4ad581c535b6ff0d77fbee3faf0a8f09f881047e65a`.
В этой итерации код UI не менялся. Четыре нативных release-кадра Spider/Strider
просмотрены через companion JPEG; это дополнение проверки поз, а не повторная
полная приёмка меню. Launcher «Полигона» выбирает этот hash, доставка 29/29.
См. [605](docs/analysis/605-planted-contact-window-acceptance-20261114.md).

- Нижний каталог имеет единственного окончательного владельца видимости:
  после проекции текущего кадра `construction::sync_visibility` исключает
  его при видимой голограмме. Проверены повторное открытие и возврат каталога.
  Исходный свежий кадр паука уже был без дублирования; это не выдаётся за
  воспроизведение старого дефекта. Конкурирующие записи из систем убраны.
- Новый финальный кадр командира подтверждает именно Commander, объект 0:
  модель целиком, заголовок «КОМАНДИР · 01 / T1» и строительные рецепты.
- HUD, голограммы, лаборатория, главное меню и вторичные страницы используют
  общие стекло, рамку, белый текст и голубой фокус. Янтарный рабочий прогресс
  сохранён. Настройки получили ту же палитру и отклик на нажатие; пустая
  часть рамки убрана. В 960×540 надписи сохраняют нативный размер, список
  прокручивается внутри панели.
- Полный набор из девяти референсов из docs/ui-optical-work.md учтён:
  углы/очередь, T-уровни/прогресс, оба чистых HUD, оптические силуэты,
  растр, PERIMETER-типографика, яркий и спокойный проекторы.
  Изображения настоящих моделей и стоимость только в подсказках сохранены.

## Свежие финальные кадры

| Состояние | Финальный preview |
|---|---|
| Паук, 1920×1080 | [release-spider](out/ui602/release-spider/0000.preview.jpg) |
| Именно командир, 3840×2160 | [release-commander-4k](out/ui602/release-commander-4k/0000.preview.jpg) |
| Завод, короткое окно 960×540 | [release-factory-540](out/ui602/release-factory-540/0000.preview.jpg) |
| Завод и реальная подсказка рецепта, 3840×2160 | [release-factory-tooltip-4k](out/ui602/release-factory-tooltip-4k/0000.preview.jpg) |
| Лаборатория, 3840×2160 | [release-lab-4k](out/ui602/release-lab-4k/0000.preview.jpg) |
| Главное меню, 1600×900 | [release-home](out/ui602/release-home/0000.preview.jpg) |
| Комната боя, 1600×900 | [release-room](out/ui602/release-room/0000.preview.jpg) |
| Справка, 1600×900 | [release-help](out/ui602/release-help/0000.preview.jpg) |
| Настройки, 1600×900 | [release-settings](out/ui602/release-settings/0000.preview.jpg) |
| Настройки, читаемый текст 960×540 | [release-settings-540](out/ui602/release-settings-540/0000.preview.jpg) |

## Проверки и поставка

20 узких UI-проверок прошли на финальных compile inputs; dev-сборка и
визуальная итерация завершились до единственной release-сборки.
На первом этапе дополнительно пройдена настоящая очередь завода с
блокировкой ввода в карту (21 проверка того этапа). Дополнительный полный
боевой debug-fixture настроек не завершён и не засчитан; подробности и
разделение доказательств — в [отчёте 602](docs/analysis/602-ui596-final-verification-20261114.md).

Release собран за 583.67 с. Свежая копия ярлыка «Полигон»
ведёт в scripts/play-range.cmd; настоящий селектор scripts/play-range.ps1
выбирает этот exe. В проверке селектора Start-Process перехвачен;
сам exe отдельно исполнил все десять финальных нативных сцен.

- Compile inputs: `a2af2dbbe926a4c5137d8682c2dfce1d23604d8e177356b573c42f5cb73563a2`.
- target/release/blast.exe SHA-256: `59edfcf22490d634cc6a3ca919c2b926b5dfe33e79bf22c52ab27b265e5e2dfc`.
- [20 UI checks](out/ui602/checks.json), [release build](out/ui602/build-release.json),
  [финальный аудит](out/ui602/final-audit.json), [выбор лаунчера](out/ui602/launcher.json).

Кадры сняты на паузе для приёмки UI. Боевой FPS этим запуском не измерялся.
Очередь пользователя, настройки контекста, архивный чат и базы Codex не менялись.

---

## Предыдущая приёмка UI 596 — 2026-11-14

Статус: проверенные UI-пути приняты. Release собран один раз;
восемь финальных нативных кадров получены и просмотрены через preview.

## Объём итерации

Общая оптическая основа распространена на боевой HUD, лабораторию,
настройки, комнату боя и справку. Верхний HUD разделён на компактные
группы навигации и состояния. Отдельная строка показывает массу своей
стороны и суммарные выработку/потребление; местная сеть остаётся в панели
объекта. Настоящие обработчики команд и рецепты сохранены.

Библиотека лаборатории использует прямые рамки с угловыми метками,
узкие заголовки и прозрачные оптические миниатюры настоящих моделей.
Комната боя разделяет стороны, справка — действия и клавиши.
Общие поверхности проявляются, кнопки получают затухающую подсветку.

В карточках рецептов только изображение и название. Подробности и
действительная стоимость находятся в отдельной подсказке. Проектор
привязан к верхнему занятому вокселю над ядром корпуса; высота соседней
башни больше не подставляется вместо крыши в этой колонке.

## Референсы

Использован полный набор из `docs/ui-optical-work.md`. Все девять направлений
повторно просмотрены через лёгкий контактный лист
`out/ui596/continued/references/contact.preview.jpg`:

1. Precision-framed RTS factory build interface — точные углы, очередь,
   подробности отдельно от ячеек.
2. d560e58a… — производственные уровни и янтарный рабочий прогресс.
3. bbc5d763… — бело-голубой свободный HUD и тихие показатели ресурсов.
4. a5e81164… — заметное наведение, сохраняющее видимость карты.
5. 900e0cca… — сдержанные сиреневые силуэты и вторичный янтарный цвет.
6. a1935349… — растр в миниатюрах и показаниях.
7. Изображение ChatGPT 10 нояб., 08_52_49 — композиция меню и типографика.
8. Изображение ChatGPT 13 нояб., 13_49_56 — проектор, лучи и глубина рамок.
9. Изображение ChatGPT 13 нояб., 13_50_03 — компактная спокойная проекция.

Финальная композиция адаптирована к настоящему кубическому миру и
управлению у объекта. Цены из концептов в ячейки не возвращены.
Оригиналы не изменены. Для общего просмотра открывались только JPEG preview.

## Найденные и исправленные проблемы

- Первый кадр 960×540 показал пересечение карточки с общей полосой юнитов
  и служебными кнопками боя. Полоса скрывается при открытой мировой
  карточке, вторичные действия доступны через «Сражения».
- Мини-карта размещается на свободной стороне, когда карточка занимает
  другую. Старое правило каталога, скрывавшее её в коротком окне,
  переопределено для этого режима.
- Положение командира в старом кадре 596 не подтверждало его карточку.
  Финальный кадр использует отдельную сцену с командиром первым объектом,
  камерой `exposed` и zoom 1,3. Командир, его ноги и карточка целиком видны.
- Стенд теста меню не создавал ресурс времени для плавной подсветки.
  Ресурс добавлен в тестовый стенд; обычное приложение получает его
  от штатных плагинов Bevy.

## Development-кадры

- `dev-factory-540/0000.preview.jpg`: повторная проверка перекрытий;
  прототип до окончательного восстановления видимости мини-карты.
- `dev-factory-4k/0000.preview.jpg`: компактные верхние группы, карточка,
  реальные миниатюры, очередь и вынесенная подсказка; открыто и проверено.
- `factory-tooltip-detail.preview.jpg`: точечный кроп 4K-карточки,
  проверены надписи, рамки и отделение стоимости от ячеек.
- `dev-lab-1920/0000.preview.jpg`: открыто и проверено; библиотека,
  инспектор и инструменты укладываются в 1920×1080.

Все пути в этом разделе относятся к `out/ui596/continued/`.

## Проверки кода

`checks.json` и `tests.log` фиксируют 19 быстрых проверок последнего
набора: навигация и возврат из меню, сохранение паузы и ввода,
раскладка карточки/подсказки, ресурсная строка, крыша проектора,
фильтры инспектора и оптические миниатюры. Число выполненных тестов
и точные имена проверены средствами `scripts/test-fast.py`.

Проверка настоящей кнопки рецепта, сохранения производственной очереди
и блокировки приказа земле прошла отдельным полным прогоном перед
последними визуальными правками. Доказательство сохранено в
`before-visual-layout-checks.json` и `before-visual-layout-tests.log`;
оно не выдаётся за новое выполнение после правки размещения.

Маленькие проверки HUD используют небольшую сцену без ненужной
подготовки всего каталога производства. Вспомогательный preview-скрипт
прошёл четыре проверки; преобразование реальных PNG проверено отдельно.
При недоступном FFmpeg используется штатный декодер Windows.

## Финальная квалификация

Development-меню и лаборатория просмотрены. Убраны неподдерживаемые
стрелки в подписях кнопок и подсказках; дублирование энергии в заголовке
HUD устранено. Финальные страницы подтверждают эти исправления.

Все восемь команд захвата завершились с кодом 0. Проверена свежесть PNG,
сохранены их SHA-256, команды запуска и SHA executable в
`out/ui596/continued/release-captures.json`.

| Кадр | Что проверено |
|---|---|
| [HUD 960×540](out/ui596/continued/release-factory-540/0000.preview.jpg) | Карточка слева, карта справа; служебные панели не перекрывают команды |
| [Завод 4K](out/ui596/continued/release-factory-4k/0000.preview.jpg) | Крыша проектора, реальные рецепты и очередь, стоимость отдельно |
| [Командир 4K](out/ui596/continued/release-commander-4k/0000.preview.jpg) | Правильный объект и ракурс, видимые ноги и доступная карточка |
| [Лаборатория 4K](out/ui596/continued/release-lab-4k/0000.preview.jpg) | Оптическая библиотека, инспектор, инструменты, исправленные подписи |
| [Главное меню](out/ui596/continued/release-menu-home/0000.preview.jpg) | Читаемая типографика, штатные режимы и выбор |
| [Комната боя](out/ui596/continued/release-menu-room/0000.preview.jpg) | Две стороны, действительные значения массы, кнопки запуска и возврата |
| [Справка](out/ui596/continued/release-menu-help/0000.preview.jpg) | Действия и клавиши, переносы, возврат без отсутствующих глифов |
| [Настройки](out/ui596/continued/release-menu-settings/0000.preview.jpg) | Параметры, вкладки и настоящие сохранённые значения, читаемые кнопки |

[Сравнение до/после](out/ui596/continued/ui-comparison.preview.jpg)
составлено только из JPEG preview; исходные PNG сохранены.

Манифест сборки: `out/ui596/continued/release-manifest.json`.
`target/release/blast.exe`: 89 189 376 байт; сборка заняла 587,96 с.
SHA-256: `b7246c082f7ed81be991694e14a7e6241adb5916195ac523013f1ac235bd5a0e`.
Fingerprint исходников и компилируемых входов сохранён в манифесте.

Ярлык рабочего стола «Полигон» проверен по копии LNK с латинским именем:
он ведёт на `scripts/play-range.cmd`, рабочая папка — этот репозиторий.
Оригинальный ярлык не переписывался. Штатный `play-range.ps1` выбирает
именно этот release, его SHA совпадает с манифестом (`shortcut.json`,
`launcher.json`).

Проверка ОС-мышью через computer-use недоступна: инструмент вернул
`Computer Use was not approved to use blast`. Ввод через этот инструмент
не выполнялся. Переходы меню проверяются штатными Bevy-тестами;
нативные кадры страниц получают через служебный `--menu-page`, который
выбирает обычное состояние меню, не подменяя его обработчики.

Снимки UI делаются на паузе с 30 FPS, двумя compute-воркерами,
одним physics- и одним nav-воркером, с ограниченным CPU и фоновыми
окнами. Они не измеряют боевой FPS. Задание по настоящему боевому FPS
остаётся в исходной очереди.

## Отдельный дефект паука

Пользовательский `out/codex-migration-20261114/spider-feet.preview.jpg`
показывает две нижние части ног/стопы, загнутые вверх к корпусу.
Это сохранённое свидетельство для задания по ногам паука/шагохода;
в UI-итерации механика ног не исправлялась.

## Mechanics delivery 597 — 2026-11-14

The unchanged UI596 layout is now included in the Strider locomotion release.
New release SHA256: ef95701e3e26967ab0ceb5554725609ea4b58d3fbee22f9418a4642e2c7e0ff9.
UI596 captures remain evidence for that UI iteration; they are not relabelled as
new captures. New native mechanics/UI renders are
out/locomotion597/release-stop/0000.preview.jpg and
out/locomotion597/release-descent/0000.preview.jpg. Both were inspected.
25 mechanics checks and flat/hill replay/worker comparisons passed; launcher
selection matches the new release hash. Details and remaining queue:
docs/analysis/597-strider-support-window-and-height-20261114.md.
These paused frames do not measure battle FPS.

## Mechanics delivery 598 — 2026-11-14

UI596 remains unchanged. The current forest/minor-stone release is
cf5da80c58c207ccd8d521935a855f67f514da282b1900b3f629de549dff1233.
43 SIM checks, 146 nav checks and 2 configuration checks passed; actual forest
traversal/replay covers ten ground models. Native release-lightning and
release-strider each provide two unpaused/realtime frames, inspected via JPEG
previews. Movement and felling are visible; no mass battle FPS is inferred.
The actual launcher selector matches this release hash. Delivery and boundaries:
docs/analysis/598-forest-and-pebbles-resistance-20261114.md.


## Walking mechanics 599 — development visual acceptance (2026-11-14)

Accepted development previews: out/walking599/dev-spider-wide/01-aim.preview.jpg
and out/walking599/dev-strider-detail/00-ready.preview.jpg. Native runs are
90 frames, realtime and unpaused; Spider uses zoom 2.2/DX12 and Strider
zoom 0.9/Vulkan. Whole-body views and the saved closer Spider pass show
consistent ankle/knee articulation in the inspected transitions. The exact
old folded pose in the supplied image was not freshly reproduced; this is
evidence for the anchor/contact-IK correction, not a blanket visual closure.

61 focused/related checks and four full-SIM restore/worker runs passed.
Spider/Strider planted slide and planted IK error are zero in those traces.
The original Vulkan baseline DeviceLost attempt is explicitly invalid; later
Vulkan development captures passed. Release qualification also passed: one
636.86 s build, both native Vulkan traversal previews inspected, launcher
hash matched, and delivery audit 25/25. Release SHA256:
`b0987a85c3ded84bb8da0f6a72b476ae920ebeda1a1369839fd26e85fd4519e4`.
Final previews: out/walking599/release-spider-turn/01-aim.preview.jpg and
out/walking599/release-strider-reverse/01-aim.preview.jpg.
Details and remaining support/source limits: docs/analysis/599-multiped-world-anchors-20261114.md.


## Walking mechanics 600 — dev visual acceptance (2026-11-14)

Inspected lightweight previews: out/walking600/dev-commander/01-aim.preview.jpg
and out/walking600/dev-walker/01-aim.preview.jpg. Both native runs use Vulkan,
90 frames, realtime, with no pause flag. Whole-body views include the supporting
sole and swing leg during turn/reverse on the terrain course; original PNGs
are preserved. This checks the biped stance/landing correction. It does not
prove general load transfer, moving rubble support or battle FPS.
61 final checks and eight full-SIM restore/worker runs passed. Release
qualification passed: one 637.64 s build, both native Vulkan previews inspected,
launcher hash matched and delivery audit 45/45. Release SHA256:
`3b851289f3b12a07eae39bf35f68ae6a0f6c0a3ece2e0bfa3500d4e15a3c6af1`.
Final previews: out/walking600/release-commander/01-aim.preview.jpg and
out/walking600/release-walker/01-aim.preview.jpg. Details:
docs/analysis/600-biped-world-anchors-and-shared-preview-20261114.md.


## Walking mechanics 601 — final dev acceptance (2026-11-14)

Inspected out/walking601/dev-final-spider/01-aim.preview.jpg and
out/walking601/dev-final-strider/01-aim.preview.jpg after the final trajectory
correction. Both Vulkan runs are 90 frames, realtime/unpaused; whole poses
include soles and knees during turn/reverse on the terrain course.
Final source passed 65 mechanics checks, 4 reporter checks and eight
full-SIM restore/worker runs. Target continuity alone was rejected as
acceptance: actual solved take-off IK is also checked. Release qualification
passed: one 638.36 s build, both native Vulkan previews inspected, exact
launcher hash matched and delivery audit 59/59. Release SHA256:
`5f30821162a63e12cc3061ecf3617e43d7cdbadb152dd5d549d3455f12ed193b`.
Final previews: out/walking601/release-spider/01-aim.preview.jpg and
out/walking601/release-strider/01-aim.preview.jpg. Details:
docs/analysis/601-continuous-takeoff-and-swing-20261114.md.

Документальный runner: первыйCargo-wrapper не вернулся после успешной2,98sсборки;
он остановлен доinventory,отказсохранёнвout/integration627g/rejected-docs-cargo-wrapper.json.
Три проверки исполнены заново черезtest-fast --no-build наранее проверенном
хешем/исходником harnessG:3passed/0failed/0ignored. Это не переиспользование
старого результата теста.
