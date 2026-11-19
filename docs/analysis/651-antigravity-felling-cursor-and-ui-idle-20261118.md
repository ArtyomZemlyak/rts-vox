# 651 — потоковый фронт валки и idle Layout UI: приёмка Round 5

Дата: 2026-11-18. Исходник до работы: `390b2196c0803b893814f4211741ad94c3fdde94bee280b98c7f9569707411da` (650).
Окончательный исходник: `a57357852a0dfca2cc0dac3f25d6eb1e1552a49b42c34a7bea0a345645b6b463`. Development binary: `7cc16196a85c1ae139aa97dad6374fee1432568a6eb275a51d2416f175873f90`,
`out/round651/blast-round651.exe`. Полный patch13 не принят: сохранена его часть,
не меняющая обслуживание физической материи. Patch14 принят после focused и нативной проверки.
Общий FPS/release/625/627 допуск этим отчётом не закрывается.

## Решение по patch13

SHA проверенных пакетов: 13 — `eed853a7f0f65279f401480c88b0e0d4ebe508f200dece208439b0400e25640c`,
14 — `069a0199c72829bb541ecffa40122eee7f7f239d3f1be3e2ac24cca24ff01b54`.
Изолированные receipts Antigravity не засчитаны как pass текущего дерева.

Полный live-only кандидат (`6f7e0fe4…`) прошёл механические проверки и сохранял CSV
Walker04, но изменял объём физической работы за тик. На ticks60..252 очередь
254121→225561, фрагменты17274→19601, coarse work/маршруты и записанные команды менялись.
Первый боевой A/B: SIM mean40.128→54.442мс; обратная пара: baseline44.995 против
candidate47.994мс. motion_felling был дешевле, но route/commit/debris дороже.
Одиночный Walker в ABBA ускорялся (trees2705→2365мс, mixed10206.5→6538.5мс),
однако это не компенсировало боевую регрессию. Доказательства сохранены в
`mechanical-comparison.json`, `reverse-comparison.json`, `surface-paired.json`,
`build-live-spread.json` и `blast-live-spread-candidate.exe` внутри `out/round651`.

В окончательном коде оставлены потоковый `Front::Cursor`, точный read-only
`can_claim_without_room`, быстрый пустой `claimed` и `MatCursor`. Отказанная пара
не удаляется и не вставляется заново. Проверка допуска учитывает присоединение,
слияние компонент с освобождением места и max_claims; окончательный claim остаётся
общим. Семена, радиус, верхнее полупространство, последовательность контактов,
прежние посещения AIR/interior и квота сохранены. Батч/live-only spread снят только
из внесённой в этом запуске дельты; прежние WIP не откатывались. `fn spread`,
физическая листва/dust0, SOLID_SAMPLES64, nav649, walking650 и AMBER сохранены.

Добавлены независимый BTreeSet oracle курсора (границы, seek, insert/clear) и
проверка фильтра допуска против настоящего zero-room claim. Первую fixture
исправили так, чтобы она действительно проверяла освобождающий мост: test-only
small_piece_pct0/100; параметры игры не менялись. Новый upstream тест девяти
live-соседей относился к отклонённой функции. Его заменил точный oracle прежних
180 пар; проверки полного падения кроны, remote foliage, radius и Ledger оставлены.
Оба промежуточных red receipts сохранены, окончательные проверки зелёные.

## Неизменность поведения и цена фазы

Все1532 строки четырёх режимов Walker04 совпали; четыре snapshots на x≈1100
побайтово равны исходнику650. Проверки arrival, материального баланса и внутреннего
restore/replay существующего driver не удалялись. За300 тиков боя855 записанных
команд побайтово равны, игровые non-time counters совпадают, конечный hash
`7c025485ae1c180c` совпадает. Диагностические arrivals/retired отдельно исключены
из сравнения: они перестали считать виртуальные remove/reinsert; visits/deferrals
и физический результат сохранены. Это855 входов, не855 завершённых приказов.
`cursor-pilot.json`, `cursor-snapshot-exactness.json`, `cursor-timing-summary.json`.

Один GPU-free sim-test A/B, stock playable_world/AI-vs-AI, workers2/physics2,
CPU0–7/Rayon2, одинаковые ticks60..252 (193 тика), без изменения workload:

| Фаза | mean / p50 / p95 / p99 / max, мс (до→после) |
|---|---|
| Весь SIM-тик | 40.128→38.388 / 35.810→34.117 / 71.691→70.407 / 100.549→98.387 / 117.712→116.673 |
| motion_felling | 3.406→1.935 / 3.362→1.891 / 5.446→2.957 / 5.866→3.160 / 6.038→3.194 |

Это ограниченный development-замер, не release и не общий20Hz допуск: p95 всего
тика выше50мс. Число ожидающих пар не уменьшилось: peak254121 в этом окне,
317159 за весь300tick бой. Эквивалентное компактное представление фронта остаётся628.

Финальная четырёхрежимная ABBA без пересборок между запусками, медианы мс:

| Walker04 | 650 | окончательный651 |
|---|---:|---:|
| bare | 161.0 | 156.0 |
| trees | 2518.0 | 2496.5 |
| rocks | 225.0 | 219.5 |
| mixed | 6319.5 | 6273.5 |

Все4 повтора сохраняют1532 CSV строки, trees78/mixed77 срубленных оснований,
max_rubble0, arrival и прежнюю скорость. Здесь ускорение практически отсутствует;
выигрыш курсора проявился в переполненном боевом фронте.

## Проверки окончательного исходника

| Набор test-fast | passed | Явные skips |
|---|---:|---|
| focused13-cursor-b | 27 | нет |
| felling-cursor | 46 | blast::matter::felling_front::tests::measure_saved_felling_front |
| suites-cursor | 125 | нет |
| movement-cursor | 83 | нет |
| ui-cursor | 44 | construction::tests::all_stock_scenarios_offer_only_their_live_allied_builders_recipes |

Это числа исполнений, не сумма уникальных tests: наборы пересекаются. Felling
покрывает физическую листву/FIFO/Ledger, стрельбу, ядерные срезы, cold restore и
worker replay. Полный movement включает76 sim-core и7 nav. UI сохраняет настоящие
timed_upgrades и production_exit. Полные debug worlds повторно не запускались.

## Patch14 и нативный осмотр

Production HUD: guarded display Detail/Meter; при наличии Progress refresh меняет
только goal, шириной владеет motion; без Progress остаётся guarded static fallback.
Проверка через настоящий builder сохраняет промежуточную ширину23% и change tick,
а затем проверяет fallback. Strategic construction bars не пишут неподвижные
Node/display/left/top. Тексты, реальные hit rectangles, семантика выбора и команды
не менялись. Соседние исправления647/644 не дублировались.

Два финальных native состояния:960×540/effects100 и3840×2160/effects0. Очередь сверху,
ниже отдельные острова, каталог не дублируется у объекта. Realtime UI clip11с:
317 кадров,16/16 RESULT valid, fan/gap пропускают ввод, deck/controls его блокируют;
minimap/Stop стабильны, object overlaps0. Клип записан с paused SIM и52 явными
acceptance ticks, поэтому не является боевым FPS.1× MP4 декодирован; осмотрены
companion JPEG и9 keyframes, не исходные4K PNG.

`out/round651/visual-review-data.json`; видео:
`out/round651/after-motion-540-100/video/realtime-1x.mp4`.

Физическая валка: cold load побайтово прежнего650 trees snapshot, затем24с
настоящего движения, ticks239..718. На осмотренных9 keyframes и двух отдельных
preview видны продолжающий движение Walker04 и падающие физические куски кроны.
`out/round651/native-forest/proof.json`, `media-proof.json`, `realtime-1x.mp4`.
Это проход одного юнита, не полноценный бой или воспроизведение исходной позы623.

## Фоновый боевой A/B

Четыре20с запуска, сначала1080 before→after, затем4K after→before. Один seed,
карта/команды/stock tuning, CPU0–7, compute/nav/physics2, cap200, profile-core на
обеих сторонах. Замороженный650 и замороженный651. Компиляция и кодирование
завершены до измеренных окон. Общее окно ticks60..275;
workload signatures equal=`True`. Стрельба/урон в
каждом прогоне реально присутствуют. Режим Fighting, не пауза.
GPU-free сценарий начинался с64 юнитов; в общем нативном окне уже73.

| Прогон | FPS | p50 / p95 / p99 / max кадра, мс | SIM× | private / working GiB |
|---|---:|---|---:|---|
| perf-1080-after | 15.253 | 63.79 / 101.26 / 129.97 / 146.58 | 0.732 | 18.493 / 15.807 |
| perf-1080-before | 13.778 | 70.95 / 106.79 / 135.89 / 157.11 | 0.689 | 18.462 / 15.784 |
| perf-4k-after | 15.010 | 64.65 / 102.88 / 130.96 / 146.26 | 0.730 | 18.823 / 15.883 |
| perf-4k-before | 13.767 | 69.40 / 109.09 / 137.91 / 151.85 | 0.688 | 18.791 / 15.853 |

| Прогон | Юниты | peak fragments | peak felling | peak world edits |
|---|---:|---:|---:|---:|
| perf-1080-after | 73 → 73 | 17545 | 277750 | 0 |
| perf-1080-before | 73 → 73 | 17545 | 277750 | 0 |
| perf-4k-after | 73 → 73 | 17545 | 277750 | 0 |
| perf-4k-before | 73 → 73 | 17545 | 277750 | 0 |

Память sampled1Hz, не VRAM. Phase/detail scopes перекрываются и не суммируются.
На окончательном source остался motor/commit: mean11.50/11.55мс и max23.23/23.19мс
в1080/4K. Отдельный felling tail1080 tick149 составляет10.249мс при11251380
thread cycles;4K tick258 —3.775мс при15994710 cycles. Квантованные thread CPU
samples0/15.625/31.25мс не дают точного разделения работы и ожидания внутри этих
коротких интервалов. Причина одиночного wall tail не объявляется установленной.
Записи сохранены в `out/round651/retained-tail-witnesses.json`; они остаются
адресными свидетелями для628/627. Максимум3.194мс в таблице выше относится только
к GPU-free прогону, а не ко всем нативным кадрам.
Одна пара на разрешение — ограниченная диагностика. Ordinary foreground FPS
остаётся открытым по выбору пользователя; эти прогоны не заменяют release,
128/1000/5000, scheduling debt и completed orders при W1/2/4/8.

## Доставка и done/remaining

Release651 не собирался и ярлык мной не переключался. Guard при заморозке
обнаружил внешнее изменение `target/release/blast.exe`: исходный627M
`967bdacc50aff659a9f485808edda7e0c73c0ca5eb36ddd263efd5faea989032` → `cf71185e844dee57926fa0c924327be091bbe22517760845123737b54443dce1`,
mtime `2026-11-18T04:54:36.862340+03:00`. Команды этого запуска использовали sim-test;
неизвестный release сохранён без перезаписи, его source/приёмка не подтверждены.
Read-only shortcut extraction вернул пустой target: launcher chain этим запуском
не подтверждён. Поэтому нельзя писать «доставленный627M неизменен».
`external-release-change.json`, `launcher-readonly.json`. Development651 от него
независим, binary/source hashes проверены в каждом native run.

| Статус | Работа |
|---|---|
| done | Review13; принят cursor/no-room/MatCursor с прежним поведением; live-only часть отклонена по боевому A/B |
| done |14 idle guards, Progress ownership/fallback, текущие focused и native проверки |
| done | Реальные четыре режима Walker, snapshots,300tick hash/commands, UI clip и физическая валка |
| remaining628 | Компактное эквивалентное обслуживание большого фронта без ускоренного выпуска материи и боевой регрессии;254k пары остаются |
| remaining623 | Точная исходная поза пользователя и root vertical drops; известный532mm случай закрыт650 и не переоткрывается |
| remaining625/627 |128/1000/5000, workers1/2/4/8, одинаковые tick windows, debt/фактически завершённые приказы, общая интеграция/release FPS |
| remaining627 | Установить provenance внешнего release cf71185e и сверить реальную launcher chain перед окончательной доставкой |

`out/round651/ledger.json`, `final-evidence.json` и адресное продолжение в
`out/queue-followups/forest-ui651-628-627.json`. App queue/steer/context limits и
архивный диалог не изменялись. Финальный ledger отдельно фиксирует завершение
моих test/build/native/media процессов; обещания фоновой работы нет.
