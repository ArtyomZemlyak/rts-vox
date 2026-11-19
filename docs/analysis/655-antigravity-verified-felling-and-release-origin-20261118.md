# 655 — verified pristine runs, deferred memo и происхождение release

2026-11-18. База654 `4bbcacc0f1057b21355631556a481aabc8e9aa1074807d2aeba436ce290cc2ab`.
Финальный source `418823c149cd04328c790ecdc1c655d2645fc9a6fed96cf64b4e01c365c9cb7e`, development SHA `3abb6810d97477a43c9107b1829f4d19c38e3ab276c57382eaab60133e2fea8b`.
**Итоговые артефакты находятся в out/round655/final.** Родительский каталог
сохраняет исходный19/source25bf7d95 и его отдельные receipts; они не подменяют
приёмку финального source418823c1. Release и «Полигон» этой итерацией не обновлялись.

## Review и перенос

19 SHA `73737e389521626387aa9d2ca367f6ddbcae98decf6c4794624411012d4f8557`
проверен и применён поверх фактического654. Пакеты17/18/19 из чужого изолированного
стека не выданы за main.17 не возвращён,18 и все остальные compile inputs
сохранены. Изменены лишь range_felling_front.rs, range_felling_front_tests.rs,
range_matter.rs. Итоговый diff/source audit лежат в final.

Front сохраняет present и transient verified masks; профиль связан с размерами
мира и нормализованным набором материалов. Вне поддерживаемого контекста hints
отключены. Материализованный чанк требует обычного reader; каждый use повторно
проверяет chunk_fresh. Жизнь генератора принадлежит World/Matter: editor reset
создаёт новый Matter, load очищает Front/hints. Самовольная смена генератора без
сброса его Matter не является дополнительным поддержанным контрактом.

Run до64 подтверждённых self-contacts применяется при pool room=0, claims=0,
без configured yielding dust, после проверки pristine для всех пересечённых
Z-чанков. Реальная версия консервативно требует claims=0 глобально; не выдаётся
за ещё не реализованный per-word claimed-span shortcut. Run не пересекает
nearby/branch head и не превышает left. Начисляются те же visits/deferrals,
after остаётся на той же последней паре. AIR и неподатливые соседи по-прежнему
расходуют исходную квоту обычного пути; spread/seed/radius/FIFO не менялись.
Сжатие работы не является фильтрацией spread_yielding, отклонённой651.

В seen_deferred найден дефект самого hash: low10 от word^(word>>17) не видит X
и объединяет соседние Y упакованного21-bit ключа. На исходном hull fixture
(z16..92,x16..32,y8..14)7296 обращений/192слова попали в2bucket: **0hits**.
Исправлено на Fibonacci multiply с wrapping и high10; в том же fixture
7104hits/192loads=97.37%. Размер1024slots и проверка полного word key сохранены.
Это измерение распределения конкретного fixture, не обещание97% для любой карты.
Технический hash не меняет игровые числа. Regression ограничивает misses двумя
compulsory working sets и ловит возврат прежнего thrashing.

## Функциональная приёмка

На одном финальном source:23focused,53movement-quick,157expanded,83movement —
passed, skips/ignored0. Только ручной ignored measure_saved_felling_front,
требующий FELL_BENCH_SNAPSHOT, не входит в focused selection; список явно сохранён
в front-selection.json. Первый собственный test-only build остановился на
неполном World namespace; исправление и исходный ошибочный receipt сохранены.

Кроме двух upstream-тестов проверены полный64-bit run, budgets0/1/2/7/8/63/64/65/
MAX, восемь nearby lanes и overflow, shifts/pop/profile/clear; отдельный тест
включает configured dust поверх verified hints и требует реальной обработки
по скалярной квоте. Это только cfg(test); игровые dust_materials остались[0].
Исходный oracle zero-repeat-reads прошёл без изменения: затем dig/put, полный
дренаж до0, равенство supplied/spent и bit volume. Подтверждённые материалы не
распылялись ради производительности.

Четыре623×620tick CSV, полные tapes и initial snapshots побайтово совпали с654.
Cold workers1/4/8 на41/301 сверялись каждый тик сW2. Walker04:1532строки и4snapshots
bare/trees/rocks/mixed побайтово равны.300tick бой сохранил 855 команд,
все нетайминговые счётчики и hash7c025485ae1c180c. Все4новых ABBA такие же.
W8 live snapshot164 (1wave/30moving/15631fragments/felling150107,57,827,649байт)
побайтово равен654; cold continuation до464 —1cb365fd26b79c0a. Replica использует
defaults restore helper, не выдаётся за ещё одну принудительнуюW8 replica.
Save/load schema и bytes не изменились, hints не сериализуются.

## Стоимость на текущем main

Четыре последовательных свежих ABBA, workers2/physics2/Rayon2/affinity8,
неизменённые binaries; ticks60..252,386наблюдений/сторону. Это optimized sim-test,
не release и не FPS. Начало64→конец73 roster, включая постройки,68alive на конце.
В этом окне одинаковы totals `{"felling_arrivals": 238794, "felling_visits": 1431004, "felling_retired": 5611, "felling_released": 0, "felling_capacity_deferrals": 1429760}`. Oracle проверяет admission.felling, а CSV — дополнительно все сохранённые нетайминговые поля.

|Фаза|До mean/p50/p95/p99/max,мс|После mean/p50/p95/p99/max,мс|
|---|---|---|
|felling_us|1.679 / 1.660 / 2.516 / 2.778 / 6.352|0.862 / 0.816 / 1.291 / 1.468 / 1.801|
|motion_us|18.210 / 17.271 / 27.407 / 32.987 / 207.558|16.304 / 16.132 / 25.233 / 29.675 / 32.146|
|total_us|40.327 / 34.083 / 74.752 / 102.933 / 221.598|37.304 / 33.133 / 65.673 / 93.814 / 105.864|

Mean changes: felling 48.63%, motion 10.47%,
total 7.49% дешевле. Сохранены все raw runs/max, без удаления
выбросов. Четыре total means в порядке ABBA: [42.98035751295337, 37.975202072538856, 36.63364766839378, 37.673015544041455]мс. Первый baseline
заметно медленнее последнего; весь агрегированный total gain не приписывается
одному19 и не объявляется устойчивым. Изолированные release3.3% и <20ms сюда
не подставлены.

|Walker04 profile,383тика/режим|До→после,мс|
|---|---:|
|bare|117→117|
|trees|2398→2459|
|rocks|148→153|
|mixed|6065→6080|

Это одна profile-пара, не доказательство устойчивого выигрыша каждого режима.
Путь, arrival, физическая масса и снесённые78/77деревьев сохранены по bytes.

## Нативные проверки и границы допуска

Один cached development build после механических gates, затем freeze. Записан
24с realtime Walker04 в лесу; original PNG не открывались, только companion JPEG.
MP41x с настоящими timestamps декодирован, contact sheet/выбранные превью осмотрены;
ручной просмотр каждого кадра не заявлен. Это сегмент реального движения и
падения физической листвы, не точное воспроизведение старой spider-feet фотографии.

Четыре20с fighting/AI background run:1080до→после,4Кпосле→до,cap200/effects100,
8CPU/2workers, без компиляции/кодирования во время замеров. Data hashes и
common-tick workload signatures совпали, окно [60, 296].
Shot/damage>0 во всех runs. Память — max1Hz samples в этом же окне, неVRAM.

|Run|FPS|Frame p50/p95/p99/max,мс|SIM×|max debt|Dropped/wall-lag,ticks|Roster|Private/RSS,GiB|
|---|---:|---|---:|---:|---:|---|---:|
|perf-1080-after|15.182|63.55/110.57/129.69/135.47|0.708113|19.0|76.89/97.28|73→73|18.51/15.81|
|perf-1080-before|14.620|66.70/112.01/130.38/136.86|0.698460|19.0|81.53/101.89|73→73|18.51/15.83|
|perf-4k-after|14.535|66.03/114.75/130.50/140.73|0.700068|19.0|80.99/101.11|73→73|18.80/15.86|
|perf-4k-before|14.236|68.27/114.05/132.25/138.98|0.691275|19.0|85.21/105.40|73→73|18.83/15.89|

Один background pair/resolution остаётся диагностикой; это не ordinary/release/
5000/no-regression допуск. Долг SIM и отброшенные ticks не скрыты за FPS.
Все core/details, workload peaks и память — battle-summary.json; phase lanes
перекрываются, складывать их нельзя. Свежие хвосты commit/feet —
retained-tail-witnesses.json. На текущем655 commit/tick280 —40.222/39.960мс,
169.6/168.4млн caller cycles(1080/4К); feet maxima1.965/1.969мс. Не приписывать
дорогой CPU commit чистому ожиданию. Tick280 и общий627 остаются отдельной
работой, уже принятой Antigravity в Round10; эта итерация её не подменяет.

## Release provenance и done/remaining

cf71185e844dee57926fa0c924327be091bbe22517760845123737b54443dce1 не изменён.
Origin закрыт как штатная пользовательская сборка scripts/play-range.ps1 -Build
по сообщению Antigravity и согласующимся артефактам: exe mtime01:54:36.862340Z,
launch tag01:54:37.306Z, GPU/window log и выход02:13:32.597286Z, depfile target.
Сами stdout/err/depfile не записывают actor/Build-switch или точный source SHA.
Поэтому exact_build_source_sha256=null; старой сборке не приписывается immutable
627M delivery и не подтверждается текущая живая launcher chain. Raw receipts651
сохранены; уточнение — release-provenance.json и обновлённые followups.

|Работа|Статус|
|---|---|
|19 + исправление hash, deterministic quota/FIFO/save/workers|done для перечисленной приёмки|
|Origin пользовательского cf7|done; exact source/immutable delivery/live chain отдельно|
|Headless ABBA и native background1080/4К|done как замеры; общий performance-допуск открыт|
|623 vertical reaction/historical screenshot|remaining;17 не возвращён|
|628/627 дорогие motion/forest tails|remaining по фактическим профилям;19 не означает устранение всех фризов|
|625128/1000/5000 с долгом SIM/completed orders,W1/2/4/8|remaining|
|Финальная неизменяемая интеграция/release/«Полигон»|remaining после общего допуска|

Очередь/steer и архивные чаты не изменялись. Все owned jobs завершены;
final-evidence.json/ledger содержат актуальный source и конкретный остаток.
