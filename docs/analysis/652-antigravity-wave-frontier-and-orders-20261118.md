# 652 — Round6: точные оптимизации wave frontier и battle orders

Дата2026-11-18. База651: `a57357852a0dfca2cc0dac3f25d6eb1e1552a49b42c34a7bea0a345645b6b463`. Окончательный исходник:
`a968a704741de36eccd8d2c9b65d78c445eb2e454f013b09a0a653ca81f892e5`. Development: `out/round652/blast-round652.exe`,
SHA256`8967de2e33d22ada86f329bee871fb739eaac20a34278ee14cefec9738b9e8f9`. Пакеты15/16 перенесены после review;
BodyMap::range исправлен до bounded binary lookup. Поведение и headless стоимость
проверены. **Общий native/release/no-regression performance допуск остаётся открытым.**

## Перенос и границы изменения

Patch15 SHA`3cf6c9b9139f150549d58c60e79c7f0b8de587e8eac0d573b0f5cdc0b03a84c7`,
patch16 SHA`f9c9218b62a451007fcc20e432c657480f1ebd818ce93f01e5298f09a920ca89`.
Оба apply-check прошли на фактическом651; эквивалентов ещё не было. Изолированный
release Antigravity не использован как pass main. Его база включала другую13;
в main сохранён принятый651 cursor/no-room/MatCursor и прежняя квота фронта,
live-only spread не возвращён.

15: расширенный proof пустых подкроновых cells, coarse read-ahead batches,
строковые masks с рабочим16×16 буфером, ленивые unpaid/confined proofs, max-price
bound, domain-clipped BodyMap с64-slot memo, точная reciprocal-декомпозиция Wave.
RangeTerrain.column использует один bin, stack до8 spans и точные ground-slice
fast paths; spill/fallback может выделять Vec. Это не обещание отсутствия любых
аллокаций во всех column-вызовах. Данные, precedence, геометрия рельефа и квоты
физического посещения не менялись. Нормальный64³ nuclear read-ahead cap сохранён.

В upstream BodyMap::range верхняя граница `index+1` переполнялась на u32::MAX;
линейный поиск конца диапазона повторно проходил длинные contact runs при resume.
Вместо него inclusive packed upper key и второй partition_point: ограниченный
бинарный поиск с прежним результатом, включая пустые диапазоны и максимальный key.
Семена/радиусы валки, размеры групп и SOLID_SAMPLES64 не менялись.

16: пропуск неиспользуемого firing_point и повторного LOS в том же решении,
локальный cache muzzle_y/old_line_clear, reuse laser bounds внутри одного
battle_orders. Акторы идут по возрастанию id; cache обновляет позы пройденного
диапазона, сбрасывается между calls и при изменении длины roster. Между лучами
тепло/щиты читаются актуально, геометрия тела меняется в своей следующей фазе.
Это не постоянный cache для произвольной последовательности мутаций/индексов.
Парные physics_laser_boxes scopes сохраняются. LOS, burn/heat, приказы и ввод
не ослаблены. Renderer/UI651 и исходные dirty WIP сохранены.

## Независимые проверки review

Добавлены legacy nine-neighbor far_ridge/range_height oracle из точного pre15
исходника (а не сравнение двух новых веток), проверки швов/радиусов; column spill
8/9/13, AIR/water/footings precedence, коротких/deep/пустых slices и course;
BodyMap диапазоны с длинными runs/u32::MAX и tight partial-wave domain на повёрнутых
и pitched hulls. Laser-cache тест теперь сравнивает и полный state_hash после
каждого луча: heat/shields/matter, помимо Trace. Первый compile выявил ошибки
новых fixtures (импорт BodyMap и несуществующий enum Factory); исправлены только
fixtures, начальный отказ сохранён в `initial-fixture-build-failure.json`.

| test-fast набор | passed |failed|ignored|
|---|---:|---:|---:|
| quick | 53 |0|0|
| focused | 29 |0|0|
| phys | 10 |0|0|
| suites | 157 |0|0|
| movement | 83 |0|0|
| weapons | 50 |0|0|
| wave-restore | 3 |0|0|

Это исполнения пересекающихся наборов, не сумма уникальных тестов. Все receipts
на одном source`a968a704`, explicit --skip отсутствуют. Focused выбирает22
обычных contact-read tests и7 дополнительных; четыре manual measurement probes
не выбирались, как pass не считаются (`contact-read-selection.json`). Phys выбирает
10 обычных wave tests, ignored frontier_cost measurement не запускался. Включены
реальный every_armed_kind wood/fire, pressure/reference/snapshot, mid-wave salvo
restore и salvo worker1/2/3/4; cached bounds отдельно имеют worker1/2/4/8 coverage.
Полный movement включает76 sim-core и7 nav. Полные debug-world fixtures не повторялись.

## GPU-free A/B и обратная пара

Неизменённый existing bench_battle, stock playable_world/AI-vs-AI,300ticks,
nav2/physics2, Rayon2, CPU0–7; SIM libraries sim-test, driver rustc-O. В обратной
паре пересборок нет. В каждом из четырёх прогонов все300 non-time counters,
855 записанных команд и terminal hash`7c025485ae1c180c` совпали. Это не утверждение
о сравнении каждого промежуточного hash. Команды записаны, а не объявлены
855 завершёнными приказами. Доказательства `comparison.json`, `reverse-comparison.json`,
`combined-headless.json` внутри `out/round652`.

Общее окно60..252,193 тика на прогон; pooled386 observations на версию:

| Фаза | mean / p50 / p95 / p99 / max, мс (651→652) |
|---|---|
| total_us | 38.214→36.865 / 34.469→32.940 / 70.609→64.701 / 98.637→89.729 / 116.908→100.043 |
| battle_frontier_us | 1.901→1.845 / 0.417→0.402 / 8.568→8.436 / 13.458→14.367 / 29.532→24.349 |
| battle_wave_step_us | 4.902→4.897 / 0.000→0.000 / 17.976→18.637 / 28.451→24.075 / 34.435→26.083 |
| battle_orders_us | 2.048→1.982 / 1.801→1.795 / 4.874→4.612 / 5.405→5.146 / 8.812→8.827 |
| battle_capture_us | 1.121→1.073 / 0.000→0.000 / 8.732→8.514 / 15.738→13.288 / 22.282→22.433 |

Mean всего тика38.214→36.865мс (−3.53%); хвосты отдельных фаз смешанные:
frontier p99 вырос, wave-step p95 вырос, orders max почти прежний. Не выдаём все
фазы за ускоренные и не переносим сюда release цифры изолированного дерева.
P95 всего тика64.701мс всё ещё выше50мс. Две одинаковые пары не являются приёмкой
разных миров/масштабов и не закрывают20Hz/full battle.

## Cold replay с реальными волнами и нативная загрузка

Existing bench_battle --replay, nav8/physics8, checkpoint на тике`164`:
волна1, движущихся30, fragments15631, felling150107. Snapshot`57827649`
bytes, SHA`6a4faaf21a6235c4a9df53b0207ac8cfafc8eb768eaa522561e348fb49bb217f`. Начальный hash restore совпал;
после ещё300 тиков (tick`464`) continuous и replica совпали:
`1cb365fd26b79c0a`. Replica использует существующий restore_snapshot helper,
workers у неё не переопределялись. Это не заявление о полном450tick W1/2/4/8 matrix.
`checkpoint-workers8/proof.json`, `run.log`, `ticks.csv`.

Один и тот же live checkpoint164 загружен в замороженные651 и652; оба native
запуска без ошибок. Два1920×1080 companion previews осмотрены, исходные PNG
побайтово равны. Снят только viewport синей базы, не вся карта и не крупный план
Siege/nuclear взрыва; это совместимость загрузки/наблюдаемого отображения.
Состояния paused, к FPS не относятся. `native-stills-complete.json`,
`native-final-comparison.json`.

## Фоновый native A/B

Четыре20с запуска1080 before→after и4K after→before. После слабого результата SIM
добавлена ровно одна контрольная1080 after→before пара. Все шесть сохранены;
вся таблица пересчитана по **одному** общему окну60..292 (233 тика), а не смешанным
окнам первичной и контрольной пары. Input hashes и workload signatures совпали.
Fighting, реальные shots/damage, профиль-core обеим версиям, compute/nav/physics2,
CPU0–7, cap200, background. Компиляция/кодирование с прогонами не пересекались.

| Прогон | FPS |p50 / p95 / p99 / max кадра, мс|SIM×|private / working GiB|
|---|---:|---|---:|---|
| perf-1080-after | 14.519 | 66.09 / 113.25 / 128.43 / 131.95 | 0.705 | 18.488 / 15.809 |
| perf-1080-before | 14.431 | 67.94 / 111.36 / 133.89 / 144.87 | 0.709 | 18.730 / 15.802 |
| perf-4k-after | 14.674 | 66.00 / 112.83 / 129.44 / 132.05 | 0.703 | 18.814 / 15.876 |
| perf-4k-before | 14.484 | 68.05 / 111.23 / 132.57 / 148.38 | 0.706 | 18.797 / 15.858 |
| repeat1080-after | 13.641 | 70.91 / 117.04 / 133.44 / 140.97 | 0.682 | 18.530 / 15.847 |
| repeat1080-before | 13.571 | 72.52 / 115.65 / 138.30 / 153.82 | 0.679 | 18.515 / 15.834 |

| Прогон |roster|peak fragments|peak felling|peak world edits|
|---|---:|---:|---:|---:|
| perf-1080-after | 73→73 | 17784 | 306668 | 0 |
| perf-1080-before | 73→73 | 17784 | 306668 | 0 |
| perf-4k-after | 73→73 | 17784 | 306668 | 0 |
| perf-4k-before | 73→73 | 17784 | 306668 | 0 |
| repeat1080-after | 73→73 | 17784 | 306668 | 0 |
| repeat1080-before | 73→73 | 17784 | 306668 | 0 |

1080 средние двух пар: FPS14.001→14.080, SIM0.693929→0.693358×. Первая пара SIM
хуже, обратная лучше; устойчивого native SIM выигрыша не показано. В4K одна пара:
FPS14.484→14.674, SIM0.705959→0.703378×. FPS немного выше, но это не закрывает
no-regression simulation допуск. Общий performance gate оставлен открытым.
Память1Hz, не VRAM. Phase/detail scopes перекрываются, их нельзя суммировать.
Measured roster73, GPU-free initial64: это не128/1000/5000 acceptance. Ordinary
foreground FPS остаётся открытым по выбранному background-only режиму; release
не измерялся. `combined-native-summary.json`, исходная4-run `battle-summary.json`
и `native-final-comparison.json` сохранены.

Оставшиеся адресные tail witnesses (`retained-tail-witnesses.json`): commit tick280
в трёх after runs38.750/39.203/40.252мс при165199980/166931160/169859030
caller cycles. Feet в исходном1080 tick236 —25.429мс при100620 caller cycles;
это не25мс доказанной арифметики вызывающего потока: отдельно нужны worker scopes
и ожидания. В контрольном1080 максимальный feet2.608мс. Квантованный thread CPU
с шагом15.625мс нельзя читать как точную стоимость короткого интервала.

## Доставка и done/remaining

Release не собирался, shortcut не переключался. `target/release/blast.exe` SHA
`cf71185e844dee57926fa0c924327be091bbe22517760845123737b54443dce1` совпадает с началом652, но его внешний provenance/launcher chain
остаются неподтверждёнными после события651; это не доказанно доставленный627M.
Development652 заморожен отдельно. `release-observation.json`.

| Статус | Работа |
|---|---|
|done|Review/перенос15/16, bounded BodyMap range и независимые oracles, сохранение651/13partial/UI/physical checks|
|done|300tick counters/855commands/hash в четырёх прогонах; W8 live checkpoint+300tick cold replay; native cross-source load/view|
|done|Текущие mechanical/AI/weapons/movement checks; GPU-free и шесть background native измерений с оговорённым результатом|
|remaining627|Native simulation/no-regression gate: SIM≈0.69–0.71×; профилировать оставшиеся measured frame/motor/capture tails без удаления проверок|
|remaining628|Эквивалентное bounded/compressed обслуживание большого felling frontier; live-only13 не возвращён и очередь не объявлена закрытой|
|remaining623|Root drops363/621мм и точная исходная поза; известный532мм случай закрыт650; Round7 не подменялся этим запуском|
|remaining625/627|128/1000/5000, workers1/2/4/8 same-input/tick-window/debt/completed-orders; финальная immutable integration/release/launcher|

Актуальные действия и witness перечислены в `out/queue-followups/wave-orders652-625-627.json`.
Queue/steer/context limits и архивный диалог не менялись. Финальный ledger фиксирует
завершённые собственные test/build/bench/native процессы; фоновой работы после ответа нет.
