# 654 — review Patch18: cut_feet, motor и покоящиеся обломки

2026-11-18. База652/653 `a968a704741de36eccd8d2c9b65d78c445eb2e454f013b09a0a653ca81f892e5`.
Текущий исходник `4bbcacc0f1057b21355631556a481aabc8e9aa1074807d2aeba436ce290cc2ab`. Development `out/round654/blast-round654.exe`,
SHA256 `188ed67e216d6dde2ef92791b572ce4bb64e95cd98e8b3865672144b9163ae35`.18 перенесён после двух исправлений границ.
Функциональная эквивалентность подтверждена; общий performance/release/627-допуск
остаётся открытым.17 не возвращён; незакрытая вертикальная реакция623 не объявлена исправленной.

## Review и исправления

Исходный patch18 SHA `f48e799a40a8d592b08e4654311d0f391e89c20655ab7db01129e88112426778`.
Apply-check на реальном main прошёл; изолированные126 проверок Antigravity не
засчитаны за текущий main. Итоговый diff: `out/round654/reviewed-current.patch`.
Изменены только `range_motion.rs` и `burst.rs`; audit всех compile inputs
подтверждает сохранность остального WIP, данных, UI, зависимостей и save-формата.

В исходном18 найден пропуск последнего X/Z ряда: column_box_ceiling принимает
полуоткрытый прямоугольник, ему передавались включённые x1/z1. Исправлено на
x1+1/z1+1. Вторая ошибка: AIR в yielding расширял резку выше прежнего ceiling;
сохранён исходный per-column потолок даже для AIR. Два независимых регрессионных
теста были RED на исходном18 (пропущена крайняя клетка[77,31,42] и лишний воздух),
после исправления GREEN. Старый bounded column reader оставлен только как
cfg(test) oracle. Он проверяет результат и квоту; production читает новый путь.

cut_feet сначала проверяет ceiling/bit volume и реальное заполнение повёрнутой
стопы; для одного попадания material_cached, для нескольких — колонка. Порядок
claim и лимит сохранены. Sorted fallback сохраняет уникальный rank/tag1..25;
первый допустимый кандидат равен прежнему минимуму. Trace использует прежний
порядок, сохраняя точный rejected. Физический прогноз80тиков и проверки не
урезаны. Lazy recovery использует snapshots прежних landing goals/contacts;
sole relief читается только когда recovery может влиять на решение. FeetScratch
держит до8 Foot на стеке, остальные на heap; tested0/1/2/4/8/9/256/257 с сохранением
байтов и независимости копии. Ранние отсевы resting/molten/Settled устраняют
лишние lookup/box_cells, не меняя их результат. Масса, физическая листва,
валка/FIFO/Ready/anchor/from и физические пороги не менялись.

## Текущие проверки и детерминизм

На одном source:5focused,53movement-quick,21debris/thermal/reclaim/restore,
157expanded(craters/supports/walking-blasts/movement-quick/AI/architecture/docs),
83movement — passed; skips/ignored0. Итоговые docs отдельно в final-evidence.
Это размеры отдельных selections, не сумма уникальных тестов. Первый запуск
focused остановлен guard из-за двух ошибочно заданных несуществующих фильтров;
после исправления списка все5 действительно выполнены. Этот tooling receipt
сохранён отдельно от двух намеренных RED boundary-тестов.

Четыре манёвра623 по620тиков: CSV, целые replay tapes и стартовые snapshots
побайтово совпали с базой653-before17. Холодные реплики workers1/4/8 на41/301
сверялись каждый тик с workers2; IK всех фаз0, up/reversed0. Ограниченный trace
на2/3/16 активировал старый fallback на313/353/381тиках: trajectory CSV и cold
hashes совпали с sorted-путём.32events/тик,512/сцену; потерь0.

Walker04: все1532 строки bare/trees/rocks/mixed и четыре snapshots побайтово
равны. В mixed существующий cold restore продолжился с другим workers-профилем.
В300tick бою все нетайминговые счётчики и 855 команд совпали; во всех четырёх
новых ABBA итог `7c025485ae1c180c`. W8 snapshot164 с1wave/30moving/15631fragments/
felling150107 побайтово равен652 (57,827,649байт); продолжение до464 и cold replica
завершились `1cb365fd26b79c0a`. Replica использует defaults restore helper, не
выдаётся за ещё одну принудительно заданную W8 replica. Данные — behavior-equivalence,
trace-after18/proof и checkpoint-workers8/proof.

## Цена работы на текущем main

Четыре свежих последовательных ABBA, неизменённые frozen binaries, workers2/
physics2, Rayon2, affinity8; ticks60..252,386наблюдений на сторону. Optimized
sim-test, **не release и не FPS**. Исходный бой64→73 объектов, включая постройки;
на конце68alive. Работа мира, приказы, волны, валка и обломки совпали.

|Фаза|До mean/p50/p95/p99/max,мс|После mean/p50/p95/p99/max,мс|
|---|---|---|
|total_us|37.314 / 33.501 / 65.587 / 90.741 / 102.822|37.089 / 32.521 / 64.987 / 90.755 / 100.722|
|feet_us|1.144 / 1.098 / 1.886 / 2.247 / 14.042|0.988 / 1.014 / 1.595 / 1.816 / 2.339|
|commit_us|9.780 / 9.159 / 16.872 / 19.778 / 21.312|9.618 / 8.955 / 16.366 / 19.169 / 21.541|
|debris_us|4.036 / 3.945 / 4.978 / 5.902 / 6.318|3.930 / 3.824 / 4.828 / 5.656 / 6.766|
|motion_us|16.962 / 17.172 / 26.033 / 29.339 / 41.400|16.597 / 16.752 / 25.204 / 29.324 / 29.998|

Mean feet -13.7%, полный тик лишь -0.6%; p99 полного тика практически одинаков.
Некоторые max выросли; таблица их сохраняет. Это не воспроизведение изолированного
release выигрыша -5.7% и не закрытие общего no-regression/20Hz-допуска.

|Walker04,383тика/режим|До→после,мс|Снесено деревьев|
|---|---:|---:|
|bare|152→115|0|
|trees|2365→2328|78|
|rocks|208→144|0|
|mixed|5922→5962|77|

Одна свежая profile-пара; small differences не считаются устойчивой оценкой.
Скорость/путь/arrival и физическое состояние неизменны. В trees почти нет
выигрыша, mixed немного дороже;628 фризы леса целиком этим пакетом не закрыты.
Старые пользовательские34/2861/12300мс другого frozen binary сюда не подставлены.

## Нативная проверка

Один cached development build83.71с, затем freeze. Реальная игра, Walker04 из
identical mid-forest snapshot:24с realtime,239..718, физическое падение листвы и
продолжение движения. MP41x с настоящими timestamp, декодирован FFmpeg; осмотрены
companion previews и contact sheet, оригиналы PNG не открывались. Полный фильм
покадрово вручную не просмотрен; точная старая поза spider-feet этим не доказана.
Кадры: `out/round654/native-forest/maneuver-contact.preview.jpg`;
видео: `out/round654/native-forest/realtime-1x.mp4`.

Четыре настоящих fighting/AI background-профиля,20с полезной записи каждый:
1080до→после,4Кпосле→до.8CPU/2workers, cap200, effects100; без компиляции/кодирования
во время измерений. Одинаковые data hashes и common-tick workload signatures.
Общее окно тиков [60, 298]; shots/damage>0 во всех запусках.
Память — max1Hz samples private/working-set в этом же окне, не VRAM.

|Запуск|FPS|Frame p50/p95/p99/max,мс|SIM×|max debt,ticks|Dropped/wall-lag,ticks|Roster|Private/RSS,GiB|
|---|---:|---|---:|---:|---:|---|---:|
|perf-1080-after|15.110|64.32/111.47/129.32/134.13|0.699658|19.0|82.42/102.17|73→73|18.74/15.80|
|perf-1080-before|14.388|67.58/112.07/130.54/136.22|0.698835|19.0|83.37/102.57|73→73|18.53/15.84|
|perf-4k-after|15.118|64.40/111.48/129.56/137.53|0.702766|19.0|81.16/100.66|73→73|18.84/15.90|
|perf-4k-before|14.848|64.84/112.09/130.71/137.08|0.698369|19.0|83.26/102.79|73→73|18.90/15.91|

Пики нагрузки по этим прогонам: `{"fragment_records": 17893.0, "corpse_records": 0.0, "live_waves": 22.0, "pending_world_edits": 0.0, "felling_pending": 313198.0, "moving_units": 31.0, "searching_units": 20.0}`.
Полные dropped_ticks, независимый wall lag, roster, core phases и память сохранены
в battle-summary.json; p95SIM batch — не p95 отдельного тика.
Background остаётся диагностикой по выбору пользователя. Один20с pair/resolution
не доказывает устойчивого native no-regression. Обычный FPS/release/5000 открыты;
пауза/стоп-кадр не использованы как FPS. Старый26.45мс feet witness650 имеет мало
CPU cycles относительно wall time: это свидетельство off-CPU/wait, но не строгое
доказательство исключительно ОС-вытеснения без scheduler trace.

Свежий устойчивый CPU witness654: motion_commit на tick280 —38.920/38.175мс
(1080/4К) и164.5/163.0млн caller cycles. Текущие feet maxima лишь1.892/2.044мс;
это не утверждение, что ОС больше никогда не вытеснит workers. Сохранившийся
дорогой commit разбирается дальше по тем же входам; данные и scope coverage —
retained-tail-witnesses.json. Не подменять CPU-стоимость догадкой о чистом ожидании.

## Done / remaining

|Работа|Статус|
|---|---|
|18 на актуальном main, две ошибки границ, focused и expanded|done|
|Траектории/материя/restore/workers, trace против sorted|done для перечисленных сценариев|
|Headless ABBA и фоновые1080/4К с реальными боем/нагрузкой|done как измерения; общий performance-допуск открыт|
|Вертикальная реакция623 и старый screenshot witness|remaining: walking653-patch17-responsiveness.json;17 не возвращён|
|628 physical forest frontier/tails|remaining: в mixed нет устойчивого выигрыша; не заменять физическую массу dust/стоком|
|625/627 масштаб128/1000/5000, SIM/debt/completed orders|remaining по прежней очереди|
|Неизменяемая финальная интеграция/release/launcher/«Полигон»|remaining после общего допуска|

Документы и local followups обновлены; app queue/steer не менялись. Release и
ярлык653→654 не тронуты. Внешний `cf71185e844dee57926fa0c924327be091bbe22517760845123737b54443dce1` по-прежнему без
подтверждённого provenance/launcher-chain с651. Ни один незакрытый gate не выдан
за готовую доставку. Все owned jobs завершены, финальный ledger и receipts —
`out/round654/final-evidence.json`.
