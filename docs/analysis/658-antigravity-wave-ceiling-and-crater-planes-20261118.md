# 658 — Patch22: ceiling memo и crater planes с независимыми оракулами

Дата: 2026-11-18. База657 `aa96a2ceaba1d5e7b20b4451eeb93bec1ee23860c5974420b60c0fde73fe9900`; кандидат `57051085b2788c77007554dd8225758547daff27f035b462cfef0bfe8d7ddcbf`.
Исходный22 SHA256 `3882924f866e4d634295a96e32e6b7946170fd291771dcd0b5949344d4031a44`.
Пакет принят после review и локального исправления. Это отдельная оптимизация blast;
gait21, nav20, range_motion, UI, data и все прежние WIP сохранены.
Patch17 не возвращён, будущий23 не применён.

## Реализация и найденный дефект

Перенесены 2D ceiling memo по колонкам волновой сетки, ограниченный parallel prefill,
консервативное сужение material_candidates, mixed crater planes и переиспользование
nuclear_soil_column. Материя, физические проверки, FIFO/квоты и формат сохранения не менялись.

Независимый RED обнаружил реальный промах partial tree cache: индекс `cx=2, cz=0`
при ширине2 попадал в следующую строку. На `[70,16,10]` реальный material5 отсутствовал
в candidates `[0,1,7,8]`. Теперь перед линейным индексом явно проверяется X;
вне кэша используется процедурный tree_cell. Полный штатный кэш сохраняет быстрый путь.

Дополнительные оракулы сравнивают прямые воксели с planes на нечётных/крупных edge,
обрезке мира, edits/AIR и неизвестном material65535; кандидаты — со структурами,
поверхностями, authored GL и partial/full cache; ceiling cache — со старым uncached
путём и новой эпохой правок. Первое предположение собственного теста, что edge1
обязан вернуть None, оказалось неверным: старый soil reader поддерживал его.
Исправлен тест через сравнение со старым контрактом, runtime/порог не ослаблен.
Confined классификация транзиентна и не сериализуется; проверяется soundness,
а не обязательное равенство степени консервативности старому пути.

## Проверки

| Пакет | Результат | Оговорка |
| --- | --- | --- |
| Baseline nuclear/contact inventory | 26 passed, 4 ignored | Четыре существующих ручных benchmark #[ignore], не скрытые --skip |
| Финальная точная nuclear/contact выборка | 31/31 | failed/ignored/skip0 |
| craters + movement-quick + supports + walking-blasts + architecture + docs | 136/136 | failed/ignored/skip0 |
| Python timeout runner | 20/20 | Плюс настоящий owned leaf sleep60 остановлен за1.047с |
| Nuclear 1100, baseline/candidate/ABBA | одинаково | Все1100 removed/queued,21полных observed hash, prestrike snapshot byte equal |
| Cold nuclear W1 | 835/835 full hashes | Завершённый subcase исходного прерванного aggregate |
| Cold nuclear W2/W8 против physics4/nav2 | passed | Все835тиков физических счётчиков;43fullhash checkpoints на каждый worker, плюс initial265 |
| Ordinary battle300 + ABBA | одинаково | Все300нетайминговые counters,855command records byte equal, hash569eb0737446cb8f |
| Active battle checkpoint W8 | passed | snapshot164 byte equal657; restore→464 hash75ade2f7de566431 |

У ядерного мира4096×192×2048,1100тиков,physics4/nav2, итог removed30258057 и
hashd61f7948e6bd9555. Точное окно cold: snapshot активной волны265→1100;
каждый тик tick/removed/removed_materials/queued/wave_counts/fired/revision,
полный hash на266..281,кратных32 и1100. Это не835fullhash на W2/W8.
Доказательства: `out/round658/checks-*/test-index.json`, `nuclear-comparison.json`,
`nuclear-abba-summary.json`, `comparison.json`, `abba-summary.json`,
`cold-nuclear-checkpoints-v2/proof.json`, `checkpoint-workers8/proof.json`.
У battle-checkpoint непрерывная ветка явно W8/W8; replica штатного replay helper
восстанавливает свои default workers, здесь не переопределена. Это отдельно от
ядерных cold-копий с явными worker_counts1/2/8 против physics4/nav2.
Nuclear CSV нумерует цикл с0; cold/snapshot tick — фактический tick симуляции.

## Замеры SIM на одинаковых входах

Оптимизированный sim-test, не release и не renderer FPS. ABBA четыре запуска,
affinity8, обычный бой nav/physics2, ядерный nav2/physics4; без конкурирующих сборок,
native и кодирования. Обычный бой64→73объекта, включая строения, не5000юнитов.
Все строки ниже в миллисекундах; фазы вложены и не суммируются с общим тиком.

| Сценарий/фаза | mean до→после | p50 | p95 | p99 | max |
| --- | --- | --- | --- | --- | --- |
| Nuclear1100/total_us | 26.427 → 26.332 | 19.566 → 19.833 | 63.640 → 63.651 | 67.383 → 66.857 | 99.229 → 88.983 |
| Nuclear1100/frontier_us | 1.086 → 0.838 | 0.001 → 0.001 | 4.337 → 3.469 | 5.364 → 4.243 | 68.893 → 10.080 |
| Nuclear1100/capture_us | 0.054 → 0.030 | 0.000 → 0.000 | 0.000 → 0.000 | 0.000 → 0.000 | 22.973 → 14.235 |
| Nuclear1100/wave_us | 3.235 → 3.349 | 0.000 → 0.000 | 8.833 → 9.354 | 9.702 → 10.242 | 40.645 → 41.153 |
| Battle60..252/total_us | 36.199 → 35.896 | 32.210 → 31.288 | 63.159 → 61.600 | 89.404 → 85.578 | 98.979 → 96.268 |
| Battle60..252/battle_frontier_us | 1.857 → 1.625 | 0.347 → 0.345 | 8.590 → 7.547 | 15.012 → 11.847 | 24.316 → 19.137 |
| Battle60..252/battle_capture_us | 1.076 → 1.077 | 0.000 → 0.000 | 8.422 → 8.573 | 12.996 → 13.661 | 21.400 → 21.582 |
| Battle60..252/battle_wave_step_us | 5.270 → 5.413 | 0.000 → 0.000 | 18.542 → 20.893 | 25.728 → 25.819 | 27.092 → 29.053 |
| Battle60..252/commit_us | 9.348 → 9.308 | 8.644 → 8.685 | 15.385 → 15.614 | 18.588 → 18.266 | 20.399 → 20.898 |


Среднее двух повторов именно witness tick258:

| Вариант | Весь тик,мс | battle_frontier,мс |
| --- | --- | --- |
| before | 97.354 | 67.844 |
| after | 39.833 | 9.783 |


В первичном pair максимум99.521→87.499мс, число>100мс было0→0;
изолированное утверждение Antigravity1→0 в этом окружении не присваивается.
Ускорение witness не означает устранение всех хвостов: p95/p99/max выше50мс
остаётся видно в таблицах. В обычном бою wave_step mean5.270→5.413мс,
p9518.542→20.893мс выросли; в Nuclear wave mean3.235→3.349мс также выше.
Эти отрицательные фазовые данные сохранены; небольшой выигрыш общего тика
не выдаётся за устойчивый общий performance-допуск. Остаточный ядерный максимум — zero-based tick587:
88.983/88.234мс в двух candidate прогонах,frontier3.682/3.538мс,
wave8.293/8.185мс,removed20667879,queued97378. Это другая ветка, не закрытая22;
её дальнейшая фазовая диагностика записана адресно вместе с motion_commit и масштабом.

## Native и background бой

Два paused4K кадра одного active265 snapshot на frozen657/658, view map,
одинаковые camera/zoom/effects0. Для осмотра сначала image-preview.py, затем
companion JPEG. Это проверка видимого мира/кратера, не FPS, не полная новая
приёмка анимации ядерного облака. Полный1100tick сценарий проверен отдельно выше.
Native proof: `native-nuclear-{before,after}/proof.json`, `native-stills-complete.json`.
Оба companion JPEG реально просмотрены: одинаковые лес/выемка/обломки/HUD,
видимого расхождения нет. Это промежуточная coarse-выемка на265,
а не финальный вид кратера1100; полная визуальная ядерная приёмка не присвоена.

Четыре самостоятельных20с фоновых fighting запуска:1080 before→after,4K after→before,
cap200,compute/physics/nav2,profile-core на обеих сторонах. Общие тики
[60, 306]; InputHashes и workload signatures совпали.
В каждом реально есть стрельба/урон, движущиеся юниты, обломки и правки мира.
CPU/memory sampled1Hz, private/working вGiB, неVRAM. SIM/debt не спрятаны за FPS.

| Прогон | FPS | p50мс | p95мс | p99мс | maxмс | SIM/wall | max debt | dropped | private/workingGiB |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| perf-1080-after | 16.385 | 60.98 | 99.24 | 119.83 | 123.19 | 0.725 | 19.0 | 73.6 | 18.50/15.81 |
| perf-1080-before | 16.143 | 61.86 | 99.18 | 120.60 | 129.18 | 0.722 | 19.0 | 75.2 | 18.55/15.84 |
| perf-4k-after | 16.179 | 62.02 | 99.59 | 119.03 | 122.55 | 0.721 | 19.0 | 75.5 | 18.89/15.91 |
| perf-4k-before | 15.948 | 62.22 | 100.63 | 120.92 | 127.17 | 0.721 | 19.0 | 75.7 | 18.86/15.91 |


Это диагностический background A/B, не ordinary/foreground/release допуск.
См. `battle-summary.json`, manifests/execution/csv/core каждого perf-*,
`retained-tail-witnesses.json`. Новый dev build заморожен один раз после логики,
кандидат source57051085, baseline sourceaa96a2ce в отдельных frozen бинарниках;
renderer/UI код не менялся.

## Отдельное исправление надёжности проверки

Исходный exhaustive cold protocol запущен11:32:25+03. W1 завершил835полныххешей,
W2 aggregate не завершён. После900с обнаружено, что timeout cleanup test-fast
мог зависнуть в taskkill/p.wait. В11:49:43.501+03 остановлена только owned session50029
черезCtrl+C. Неразрушающий open собственного exe подтвердил unlock, байты не менялись.
Aggregate не объявлен pass. Первый replacement driver имел compile-only ошибку:
Battle не содержит shots/damage; corrected v2 сравнивает actors.fired/units.revision.

В test-fast taskkill теперь ограничен10с, затем direct owned Popen handle kill/join
также ограничен; ошибки cleanup явно сообщаются. Общих process-name kills и
автоматического restart loop нет. При недоступном tree cleanup потомки не объявляются
проверенно закрытыми. Реальный smoke — заведомый leaf без потомков, завершён и joined.
20Python tests и логи сохранены в tooling/. Старые SIM receipts сохраняют прежний
runnerSHA4dd8e62e; новые документационные — d7014daa. Engine source не смешан.

## Done / remaining

| Область | Статус | Конкретная дальнейшая работа |
| --- | --- | --- |
|22 blast ceiling/planes + partial-cache bug | done | Локальные оракулы,1100tick equality,cold restore,ABBA/native выполнены |
|test-fast bounded timeout | done |20unit + real leaf smoke; исходный aggregate честно отмечен interrupted |
|623 | remaining | Историческая последовательность фото и остаточная вертикальная реакция; не подменять нулевым IK |
|628/627 motor tail | remaining |35candidate terrain_pose / tick280;23 ожидает отдельного review, здесь не применён |
|625 nuclear residual / scale | remaining |Nuclear tick587 ~89мс;финальный кратер/облако4K и множественные взрывы не присвоены paused265;128/1000/5000 одинаковые inputs/tick-window,W1/2/4/8,приказы/memory/SIM debt |
|ordinary FPS / release / immutable627 | remaining |Foreground не разрешён; один финальный release и launcher chain только после общих gates |

Release `cf71185e844dee57926fa0c924327be091bbe22517760845123737b54443dce1`
и «Полигон» не менялись. Origin пользовательской сборки установлен ранее, это
не immutable627. Queue/steer/context limits/архив/базы приложения не менялись.
Следующая работа записана адресно в local followup, not_enqueued, порядок queue сохранён.
