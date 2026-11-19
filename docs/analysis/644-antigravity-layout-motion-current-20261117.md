# 644 — Antigravity UI/layout/motion на актуальном коде

Объединённый03 проверен по SHA256
`72a4ec1e32fd54a70c3d2eaeeae75ec7b3f5fff0bea2b3240360948b30e275fc`.
Его база627M отличается от текущего642. Старые45+5 isolated passes не
используются как текущая приёмка; старый diff не накладывался целиком.

## Реализация

- Guard перед изменениями Node/Text/ImageNode в refresh/responsive/visibility/
  tooltip и projection update. Root height имеет одного владельца responsive;
  refresh больше не сбрасывает её каждый кадр. Сохранены размеры/острова642.
- Projection part рассчитывает желаемый Node локально и применяет set_if_neq;
  previews сохраняют прежний extraction guard NONE→WHITE. Скрытая Scan больше
  не передвигает top в отсутствие production work.
- Возраст предыдущих эффектов обновляется в prepare до новых событий;
  prepared flag сохраняет самостоятельный render. Сохранён более новый
  Duration-контракт635,без возврата f32-age из старого03.
- Part::Deck max_height использует dock_height(viewport). Числа180/292 в
  исходном review относятся к627M; более новая protection/quote geometry642
  сохранена,projection_viewport не изменён.
- Pixel selection/focus/production снова AMBER. Обычная база остаётся холодной
  navy,Optical controls и проекции —cyan. Чистые silhouettes,overflow и
  композиция меню642 сохранены; семантическая матрица обновлена.
- Construction/tests.rs не менялся: реальный timed_upgrades,три PowerPlantT1,
  sim.step/restore T2–T4,Engineer→GrebenMode→Factory и queue_row None уже640.
  Сильные zero-delta/exact endpoints и reverse owner tests635/641 не дублировались.

Desktop recipe shelf и обычная Row взаимоисключающие; compact использует один
drawer. При потере выбора скрываются отдельные shelf/tooltip roots. Дочерние
controls сохраняют authored display под скрытым родителем.

## Воспроизведение и focused acceptance

На исходном коде добавленные idle/birth regressions дали0/2: Node менялся на
тихом кадре,свежий Hover уже имел65ms возраста. После исправления2/2 прошли.
Расширенный visible-catalogue test дополнительно нашёл скрытую Scan,которая
меняла top. После guard прошёл активный builder test,включая текст/previews.

Idle test строит настоящий chrome+construction (>80 nodes),проверяет Node,
Text и ImageNode timestamps при positive/zero delta на540p/1080p/4K.
Birth test проверяет Hover/Unhover при65/80/90/100ms: первый кадр age0,
видимые strokes,точное завершение. Существующий builder test дополнен
открытым/закрытым каталогом,сохранением More и закрытием shelf при потере выбора.

Первый native прогон выявил мою ошибку перебазирования: слишком широкий
fallback скрывал дочерние ToolsToggle/FoldToggle до первого появления.
Static кадр показывал отсутствие «Ещё»,motion route сообщил missing targets10/11.
Скрытие сужено,явный assert More добавлен. Неудачные source1f99240b и captures
сохранены и исключены из финального допуска; старые passes не перепривязаны.

Финальный source:
`32a7f0aa8bc5e4a49acc002011b94a4ddc8c41598bf45948209c66817e9aeae7`.
Development executable:
`2dacf5a5bf890f6ef82602dafc7f6d9f65d83af25cb0c08bb1992df879a39a68`.
Заново68/68focused,12/12architecture,3/3docs,failed/ignored0.
Полный stock-world sweep явно исключён и записан в receipt.
Python/runner не менялись,их старые прогоны не объявляются новыми.

Scope audit восстанавливает saved UI digests только в памяти и получает
baseline642 `df596f4e…`; SIM/nav/physics/data/баланс неизменны.
Исходное рабочее дерево сохранено в out/ui644/source-before; инкрементальный
diff и receipts находятся в out/ui644. Старый UI native/battle gate642 не
подменяет новые запуски.

## Native acceptance

На том же source32a7 прошли accepted2-map540p100%,1080p35%,4K0%; все три
companion JPEG осмотрены напрямую. Ранее просмотренные matching captures642
относятся к точному executable baseline,а не к другому SIM-исходнику.

| Realtime scenario | Результат |
|---|---|
|after-motion-540-100-attempt2|318frames,16input events,error0;More/fold/catalog/hover/world gaps иblocking|
|after-production-1080-35|358frames,ticks0–219,настоящий stock Tank завершён|
|after-nav-1080-35|325frames,error0,menu/room/settings/lab ивозврат|

Все три видео1× закодированы по wall timestamps и декодированы. Восемь
актуальных ключевых companion JPEG осмотрены напрямую; покадровый ручной
просмотр каждого видео не заявляется. Native geometry содержит1394+1765
samples с максимумом0 уobject/recipe/lab overlaps. Пауза и запись не FPS.
Точные receipts,geometry-summary иnative-summary находятся в out/ui644.
Новый runner для SIM не создавался: использованы прежние test-fast/native routes.

## Фоновый UI-cost diagnostic

Сохранены шесть20-second AI-vs-AI runs:before→after1080p,after→before4K,
затем один обратный after→before1080p из-за неоднозначного первого хвоста.
Все наблюдения сохранены. Общие фактические ticks60–267; одинаковые входы,
worker counts2,affinity8CPU,Rayon2,cap200,Vulkan; background=true,без паузы,
capture/encoding. Workload signatures/selection/roster совпадают на общих
sampled ticks. Реальные выстрелы и урон подтверждены в каждом запуске.
Roster73→73,пики fragments15658,moving33,searching22,waves4,corpse0,
pending edits0 в sampled rows. Это не масштабная5000unit-проверка.

| Run |FPS|Frame p50/p95/p99/max,ms|SIM ratio|Private/working GiB|Debt max / dropped ticks|
|---|---|---|---|---|---|
|1080 before1|12.550|78.72/115.50/158.70/183.16|0.6275|18.329/15.666|19/105.39|
|1080 after1|12.065|78.82/142.34/179.03/197.36|0.6032|18.346/15.685|19/116.24|
|1080 after2|12.853|76.51/112.38/159.09/179.05|0.6426|18.310/15.654|19/96.21|
|1080 before2|12.720|78.04/112.56/161.74/181.48|0.6360|18.342/15.680|19/99.84|
|4K after|12.803|76.66/114.88/161.40/177.26|0.6401|18.627/15.716|19/97.43|
|4K before|12.747|77.25/113.30/158.32/181.92|0.6374|18.676/15.700|19/99.38|

Memory:1Hz private/working samples внутри того же tick-window,не VRAM.
SIM ratio — первые endpoint markers; SIM-phase quantiles относятся к batch
тиков в render frame. Полные phase/timing данные и100% исходные наблюдения:
combat-summary.json,combat-first-pairs.json,first-pair-phases.json.

Первый after1080 имеет плохой хвост; тот же executable в обратном повторе
его устойчиво не воспроизвёл. SIM batch p95 там102.59ms против86.97ms уbefore1,
при повторе87.69ms против88.06ms уbefore2; одновременно различаются render
tails. Причина первого выброса не установлена. Новые повторения до получения
«зелёного» результата не выполнялись. FPS/no-regression pass **не дан**:
отдельная задача627 — объяснить и воспроизвести этот хвост на одинаковом
tick-window до общего допуска. Тихие Node/Text/Image и первый motion-frame
подтверждены прямыми regressions,но они не означают real-time бой.

## Состояние доставки

Выполнена одна release build за643.377с,code0,source_stable=true.
Binary:`2f0dd22200bfe0d961b67e058b07080c85fd49b161f3de709f6bebc56c76e98a`.
Постоянная копия:out/ui644/blast-ui644.exe; запускать из корня repository
со штатными assets/data. Копия и target executable имеют одинаковый SHA256.
Три реальные smoke captures540p100%/1080p35%/4K0% прошли,все companion JPEG
осмотрены напрямую. Итого осмотрены3dev static+8realtime keyframes+3release JPEG.

| Работа | Статус |
|---|---|
| Guarded idle layout/text/previews +responsive height |done,hidden/active и3viewport regressions|
| Birth-frame motion +Duration endpoints/reverse |done,существующее не дублировалось|
| Deck max_height и сохранение protection geometry642 |done|
| Construction timed upgrades /stale selection |уже640,файл не изменён,актуальные тесты прошли|
| Amber Pixel selection/navy/Opticalcyan и mutual exclusion |done,unit/native|
|68focused+12architecture+3docs и native scenarios |done,точные receipts вout/ui644|
|Одинrelease candidate +3smoke |done,тотжеsource32a7|
|FPS/no-regression/global delivery |remaining627:необъяснённый1080tail;ordinary FPS открытпорешениюпользователя|

UI fixes завершены как functional/visual candidate. Ordinary/release battle FPS
не квалифицирован; canonical«Полигон» остаётся627M,его executable hash
`967bdacc50aff659a9f485808edda7e0c73c0ca5eb36ddd263efd5faea989032`
проверяется при финальной сверке. Launcher не переключался. Все собственные
игровые,compiler иencoder subprocesses завершены по receipts; фоновой работы
после завершения не заявляется. Final evidence иdone/remaining:
out/ui644/final-evidence.json,out/ui644/ledger.json.

Оба сравниваемых development executable содержат ранее отклонённый638
foliage sink: это сравнение только UI. Оно не выполняет628 и не допускает
общую доставку. Остаются628 физическая агрегация,625 масштабная матрица,
623 точная Spider-поза и627 общая интеграция; ordinary foreground FPS открыт
по решению пользователя. Queue/steer/context limits и canonical«Полигон»
не меняются; чужой WIP не откатывается.

## Дополнение: поздний03-v2 после завершения644

SHA25603-v2 подтверждён:
`1a30f169b7474b4506a8cb9dc91c4dec05145c47d22688e449a9c27e0cdc8142`.
Переданный46/46 receipt относится к isolated source3715cfe4,не кmain32a7.
В текущем644 уже есть тот же порядок tick-before-emission иоднократная
защита render-only черезprepared (вv2 называетсяticked). Duration,все contract
tests истрогая проверка обоих Hover/Unhover при65/80/90/100ms сохранены.
Layout/construction измененияv2 также уже учтены или перекрыты более новым642/644.

На фактическом неизменном source32a7f0aa повторный git apply --check03-v2
даётrc1;01/02 изпредыдущего review такжеrc1. Это более позднее дерево после
переноса эквивалентов,поэтому старое сообщение rc0 не является проверкой его
нынешнего состояния. Патчи повторно не применяются. У01 остаётся известный
отдельный early empty-skip guard вtooling639;02 закрыт более сильными support
проверками637. Это не меняет порядок очереди.

Этот дополнительный review не редактирует runtime/тесты и не запускает новые
build/test/native/FPS. Проверен актуальный source hash,он точно совпадает с
исходником существующих68/12/3 receipts644; isolated46 не перепривязан кнему.
Машинная запись:out/ui644/v2-handoff-review.json.
