# 660 — Иерархия production HUD, лёгкие optical recipes и янтарные pixel filters

2026-11-18. Запрос: продолжить UI. База659 `ab3788208c02243de543e8d65275edd76edf877a3dd6edcb699fc925ff5788ce`;
финальный кандидат `64951ca2936aca1b96b5f0c87d99c30ca6d95657d2e0a15ee5dee53682812088` плюс отдельный WGSL digest в shader-provenance.json. Проверены актуальный ui-optical-work,
ui-style-system и все9референсов на общем contact sheet, затем финальные647кадры.
Старое предложение заменить семантический AMBER на CYAN не переносилось:
оно противоречит прямому требованию пользователя. Палитра сохраняет navy/amber
боевого pixel HUD,cyan проекции,lavender вторичного акцента,red угрозы.

## Изменение

| Режим | Применённый регистр |
| --- | --- |
|Верхняя очередь|Pixel:тихий заголовок и disclosure,отдельный производитель,главный текущий заказ/процент,следующие элементы и янтарный progress|
|Проекционные рецепты|Optical/cyan:прозрачные самостоятельные chips,угловые контуры,чистые модельные превью и небольшие номера|
|Нижние категории|Pixel:спокойные неактивные rails,янтарный phosphor выбранного,прежние команды и якоря|
|Лаборатория/настройки|Прежний тихий white/cyan service;общий тёмный skin не накладывался|
|Главное меню|Прежняя композиция и CRT-переходы;проверены в realtime-возврате|

Заказ больше не теряется в одной строке с длинным именем производителя.
В агрегате виден первый заказ и число других линий;полный список остаётся в
существующих paged details с реальным выбором производителя. Нижняя очередь
или копия очереди в проекции не добавлялись. Содержание очередей/стоимости/
оплата/отмена/SIM не изменялись. Это повышение иерархии, а не уменьшение любой
ценой:collapsed960queue остаётся269×111physicalpx наx675/top12.

Optical chips используют один вариант прежнего bounded материала:полная
рамка заменена угловым контуром,основа прозрачнее,scan/lens detail спокойнее.
Новых проходов,зависимостей,текстур мира и fullscreen postprocess нет.
Иконки читаются из прежних cached реальных моделей;форма шаблонов не менялась.
Рецептные hit rectangles и команды сохранены;внешний вид не вводит новую
геометрию ввода. Desktop optional shelf и нижний каталог остаются взаимоисключающими;
на540p используется один drawer. Emitter/fan/parallax и lifecycle320/220 сохранены.

Нативный первый просмотр показал,что inactive filters ещё обводились самим
procedural shader. В финале только эти phosphor-surfaces приглушены в покое;
selected/hover сохраняют реакцию,selected pixel glow теперь янтарный.
Буквы не искажаются,не двигаются и не расщепляются. Native state colours и
информация сохраняются при0effects;текстовые и командные handlers прежние.

## Тесты и источники

| Receipt | Результат |
| --- | --- |
|checks-ui-first/test-index.json|71passed/failed0/ignored0;предварительный source66e9adea|
|checks-ui-final/test-index.json|71passed/failed0/ignored0 нафинальном64951ca2|
|Явный skip в обоих receipts|construction::tests::all_stock_scenarios_offer_only_their_live_allied_builders_recipes|

Skip — прежний дорогой полный stock-world sweep,не скрытый красный тест и
не pass. Остальные construction tests выполнялись с настоящими timed upgrades,
питанием/оплатой/restore и production exit;также пройдены production_hud,
spatial_ui,ui_motion contracts, optical_ui и геометрия click-through islands.
Сборка sim-test оптимизирована,affinity8/Rayon2. Старый first source/бинарник/
кадры оставлены отдельно и не присваиваются финальной версии.

## Native visual/input приёмка

На финальном source сняты960×540/100%,1920×1080/35%,3840×2160/0%,
открытый/закрытый каталог завода и Spider без каталога строителя.
PNG сначала проходили image-preview.py;реально просмотрены JPEG и contact sheets.
Финальные статичные кадры: `out/ui660/final-after-*/capture.json`,
`final-static-contact.preview.jpg`;также отдельно просмотрен полный540p preview.
Точечный UI crop для сравнения находится в `queue-projection-comparison.preview.jpg`
(слева659,справа660);оригиналы не изменены.

Три11секундных realtime1×ролика на прежнем native input runner:
540p100%,1080p35%,540p0%. Каждый декодирован и имеет16/16 valid input results,
errors.txt пуст. Проверены queue disclosure,настоящий Choose/Cancel,hover цены,
Fold/Restore,смена выбора,прозрачные fan/gap,menu blocking и Resume.
Minimap/Stop имеют стабильные rects;protected actor overlaps0 и recipe overlaps0
во всех geometry rows. При540p базовые надписи не уменьшались ниже native size,
минимальный essential panel font в geometry13px. Маленькие номера chips12px
декоративны и не заменяют имён рецептов.4K работает с UiScale2.

`visual-review-data.json` содержит пути к видео и выбранным кадрам;
`final-motion-540-100.preview.jpg`,`final-motion-1080-35.preview.jpg`,
`final-motion-540-0.preview.jpg` просмотрены. Использованы реальные wall timestamps,
без ускоренного playback. В коротких motion clips completed/produced0;
полное завершение производства проверено focused SIM/UI contracts,не придумано
по этому ролику. Paused/UI-motion кадры не являются боевым FPS.

## Цена в полноценном фоне боя

Четыре отдельные20секундные fighting runs на frozen659/final660,
1080before→after,4Kafter→before. Cap200,compute/physics/nav/Rayon2,affinity8,
BelowNormal,stock tuning,profile-core и --object-catalog на обеих сторонах.
Прежний battle driver меняет выбор живого союзника каждые60тиков;
shelf запрошен,но его controls видимы только у подходящего выбранного объекта.
Все UI-видео/preview encoding закончились до измеряемых окон;
точечный comparison crop создан после завершения замеров.

Общие тики[60, 307],InputHashes и workload signatures совпали.
Реальная нагрузка в окне:73объекта,включая строения;peak18390фрагментов,
22волны,327289pending felling,31moving/20searching. Во всех полных прогонах
записаны786выстрелов и136124damage;паузы нет. Колонка completed в battle.ui.tsv
относится к UI-motion audit,не к завершённым приказам/производству.

| Run | FPS | p50мс | p95мс | p99мс | maxмс | SIM/wall | max debt | dropped | private/workingGiB |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| perf-1080-before | 16.309 | 61.39 | 99.31 | 119.73 | 123.09 | 0.722 | 19.0 | 75.8 | 18.49/15.81 |
| perf-1080-after | 16.216 | 61.76 | 99.68 | 118.96 | 124.77 | 0.723 | 19.0 | 75.3 | 18.52/15.84 |
| perf-4k-before | 16.248 | 61.33 | 99.91 | 118.53 | 123.40 | 0.722 | 19.0 | 75.8 | 18.86/15.92 |
| perf-4k-after | 16.251 | 61.08 | 99.26 | 120.01 | 125.75 | 0.722 | 19.0 | 75.7 | 18.88/15.94 |

Одна пара на разрешение — диагностика,не статистическое доказательство
отсутствия любой регрессии.1080FPS−0.57%,4K практически одинаков;отдельные
p99/max колеблются в обе стороны. Память sampled1Hz,неVRAM.
SIM остаётся~0.72×;это не real-time и не ordinary/foreground/release FPS-допуск.
Source `battle-summary.json`,perf-*/manifest/execution/csv/ui.tsv/core.
Общий source_hash не перечисляет данный WGSL через concat-макрос:Cargo отслеживает
его как compile input. Поэтому отдельно проверены SHA256 файла и точное наличие
его исходных байтов в каждом frozen before/after.exe;shader-provenance.json
связывает шейдер с уже использованными binary SHA. Source64951ca2 здесь всегда
читается вместе с этим отдельным shader digest,не как полный хеш любых assets.
5000units и completed-orders/scaling W1/2/4/8 этим прогоном не проверены.

## Done / remaining и доставка

| Область | Статус |
| --- | --- |
|Иерархия очереди / спокойные categories / optical chips|done в пределах660;проверены код,реальные кадры и input|
|71focused / native16×3 / effects100/35/0|done;один явно перечисленный stock-world skip|
|Тепловые карты659,walking и engine/data|сохранены без изменений;проверены compile-input digests|
|Главное меню / service modes|сохранён существующий стиль;новая полная приёмка всех lab tabs этим пакетом не присвоена|
|Полный stock-world sweep / native production completion|остаются в общей интеграционной приёмке;не подменены коротким motion clip|
|Ordinary FPS,128/1000/5000,627immutable delivery|прежние открытые gates;этим небольшим A/B не закрыты|

Финальный проверенный dev: `out/ui660/blast-ui660-final.exe`,SHA256
`7f7a658455e2e8ae7561f549120f1a515155fd88292c8930bf7f7576e012f819`. Изменены только5UIcompile inputs:
construction.rs,production_hud.rs,spatial_ui/chrome.rs,optical_ui.rs/optical_ui.wgsl.
Theme AMBER,ui_motion,input/SIM,lab и menu layout source не переписывались.
Release `cf71185e844dee57926fa0c924327be091bbe22517760845123737b54443dce1` и «Полигон» не обновлялись:сохраняется общий627gate.
Queue/steer/context limits/архив/базы приложения не менялись.
