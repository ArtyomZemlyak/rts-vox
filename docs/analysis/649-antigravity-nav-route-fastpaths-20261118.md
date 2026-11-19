# 649 — Round 3: route/corridor/pose fast paths на текущем648

2026-11-18. База648: `41519e2a8e8eb252596b120ee8966f47a337a8bb08d3febab8c48285f0080227`.
Итоговый compile source: `49462b4bddb05ca7678191b1e946454e0f88d930872fcb8958ec3ad1c2312c85`. Frozen development:
`out/round649/blast-round649.exe`, SHA256 `8262c9d66a7b5646157e37901d93773cf5341c42236be5dc8f45f543c8660a5c`.
Входной patch10 SHA256 `1dcb95a9a9a5390798ef9a1b7403fdfc62ffc85063d6316544a96b04d843f020`
проверен до переноса. Старые pure645+07 timings/165tests не объявлены текущей приёмкой.

## Review и реализация

- geometry_accepted/corridor_accepted повторяют только геометрический admission.
  Full TransitionProof, source/span/DropDown/required_lift/first_refusal и
  обязательства live motor сохранены. Два пустых Vec::new сами не выделяли heap:
  здесь устранено конструирование большой proof-структуры в boolean hot path.
- Prepared floor passage применяется от площади16 tiles при no_debris.
  Сертификат использует4x4 blocks; None сохраняет точный fallback. Explain
  сохраняет полный ordered отказ. Проверено, что Ground.refusal читается только
  объясняющим путём. DebrisTiles.may_touch может дать false positive, не пропускает
  настоящий tile overlay; live dirty/reclaim поведение не заменено кешем навсегда.
- Probe scan:512-byte стек для≤256 материалов, heap fallback для больших колонок.
  Tile mask:1024 bits на стеке и heap words сверх этого, вместо Vec<bool>.
  Median5 — прежняя9-comparison сеть, вынесенная в inline helper для проверки.
- Hunt сохраняет old_cost/source между доказанно одинаковыми Lookup-запросами.
  Lookup не вытесняет source, target insertion не меняет distance до сравнения.
  Количество logical queries/tiles и сроки bounded searches сохранены.

К raw patch добавлены пять focused проверок: median5 на3125 входах с повторами
и крайними i32; колонка255/256/257/520 с AIR-дырами и rubble; tile mask ниже,
ровно и выше1024bits, с поворотом, edits/remove и независимым full-pose oracle;
skinny/малые corridor площади15/16/17 с prepared/edit/rubble и explain;
дробные Fx/крайние значения и custom lift/drop policy против полного proof.
Фикстуры требуют реального попадания в обе ветки буферов и меньше column reads,
а не только совпадения красивого кадра. Full-проверки/материя не удалялись.

Первый расширенный test build использовал неверные Fx::from_raw/тип blocked;
исправлены только tests. Затем проверка mask coverage выявила отсутствие случая
ниже1024 в самом fixture: добавлен размер119, assertion сохранён. Эти receipts
оставлены в checks-focused-final/finalb; это не обнаруженные runtime-дефекты10.
Первый вызов full-nav helper без обязательного --filter отклонён CLI до тестов;
исправлен helper. Итоговый --filter '::' выбрал весь inventory171, filtered0.

## Текущие проверки

Baseline movement-quick47/47 до изменений. На одном итоговом source:
focused **9**, полный nav lib **171**,
suites movement-quick/supports/walking-blasts/architecture/docs **102**,
full movement **80** — passed, failed/ignored/skip0.
Сохранены cold restore, prepared-ground workers1/2/4/8, deep-pit/underfoot,
Ready/planted и реальные перекрывающиеся взрывы. Заключительный docs receipt
после этого отчёта хранится в checks-docs-final.

## A/B существующих headless drivers

Unchanged bench_vehicle_pose и bench_battle, sim-test library, rustc-O drivers.
Навигация2/physics2, CPU affinity8, Rayon2 на обеих сторонах. Первоначальная пара
и обратный повтор готовыми бинарями; не wall-clock FPS. Текущее route до
патча4,855мс, а не12,434мс чужой базы. Первая пара ниже сохранена полностью.

Все300 не-временных tick counters и command CSV совпали побайтово; конечный hash
`7c025485ae1c180c` совпал. Logical coarse work **105907099**, его max/ticks/
in-flight также одинаковы. Это не full state_hash каждого тика между сборками:
такого ряда driver не записывает. Workers/restore проверяются отдельно тестами.
Pose:все4 случая сохраняют columns/scans/surfaces/hash, включая13216718dd84439d
на high heap. До/после timings каждого pose batch оставлены в comparison.json.

| Метрика | До648 | После649 |
|---|---:|---:|
| Средний тик, мс/тик | 41.092 | 44.044 |
| motion, мс/тик | 19.675 | 20.518 |
| route, мс/тик | 4.855 | 4.307 |
| p50, мс | 36.738 | 38.936 |
| p95, мс | 82.234 | 86.983 |
| p99, мс | 115.534 | 122.488 |
| max, мс | 125.437 | 132.112 |

Coarse pass total: **494175→288626мкс**;
max **11652→7194мкс**. Фазы пересекаются,
их нельзя складывать. Исходные CSV/logs и hashes бинарей лежат в before/after.

В первой паре полный тик вырос41,092→44,044мс одновременно с подорожанием
неизменённых фаз; поэтому выполнен обратный after→before без новых сборок.
Все четыре серии сохраняют те же300 counters/commands и terminal hash.
Две пары в порядке before/after/after/before: средние двух run-statistics,
не новая оценка квантилей объединённого распределения:

| ABBA,2run/side | mean tick,мс | motion,мс | route,мс | p50/p95/p99,мс | coarse pass,total мкс |
|---|---:|---:|---:|---:|---:|
| before | 41.880 | 20.073 | 4.952 | 37.183 / 83.239 / 117.593 | 501862 |
| after | 42.724 | 20.040 | 4.202 | 37.942 / 85.445 / 118.142 | 280146 |

Сырые результаты каждой серии и max сохранены в paired-summary.json; первый
неудачный полный-tick результат не скрыт. Route/coarse improvement не считается
автоматически full-battle/no-regression допуском.
ABBA даёт route4,952→4,203мс (-15,1%) и coarse pass501862→280146мкс (-44,2%),
но полный тик41,880→42,724мс (+2,0%). Общий performance pass не выдан.
Разбивка route/commit/total по окнам1–59,60–290,291–300 сохранена в route-windows.json.

## Настоящий бой в background

Четыре20s native runs:1080 before→after,4K after→before, профильcore на обоих,
workers2/CPU8/Rayon2/cap200. Baseline — frozen648, не новая сборка старого исходника.
Кодирование/тесты/сборки закончены до измерений. Fighting, реальные shots/damage,
правки мира и обломки подтверждены. Общий tick-window **[60, 290]**;
workload signatures equal **True**.

| Режим | FPS | p50/p95/p99/max кадра, мс | SIM/20Hz | max debt,ticks | dropped,ticks | private/working GiB |
|---|---:|---:|---:|---:|---:|---:|
| 1080-after | 13.658 | 70.14 / 112.48 / 144.23 / 157.20 | 0.683 | 19.00 | 87.31 | 18.48 / 15.80 |
| 1080-before | 13.541 | 72.24 / 113.36 / 143.02 / 158.66 | 0.677 | 19.00 | 93.11 | 18.52 / 15.84 |
| 4k-after | 13.634 | 71.12 / 113.86 / 144.96 / 156.44 | 0.682 | 19.00 | 88.89 | 18.81 / 15.87 |
| 4k-before | 13.683 | 71.69 / 113.35 / 142.64 / 161.83 | 0.684 | 19.00 | 87.24 | 18.84 / 15.85 |

Нативный FPS практически не изменился:1080p13,541→13,658,4K13,683→13,634.
Обе стороны имеют тот же workload:73 объекта, до31 движущегося,21 волн,
17748 fragment records,304268 felling pending в общем окне. SIM≈0,68×,
следовательно не реальное время. В записанном native profile средний
motion_commit1080p12,324→12,111мс,4K12,211→12,200мс; motion_routes_ground
3,349→3,352 и3,331→3,362мс. Это отдельная фаза, не сумма всего route_us.
Сильной регрессии native commit не видно; общий FPS-выигрыш тоже не доказан.
Новых экспериментов с удалением проверок/мира ради цифры не проводилось.
Полные roster/moving/waves/fragments/felling peaks, debt,1Hz process memory
(не VRAM) и пересекающиеся core phases: battle-summary.json. Это64→73-unit
диагностика, не масштаб5000 и не ordinary/release FPS допуск. Полные завершённые
приказы на128/1000/5000 и реальное время остаются частью625. Паузы/UI-кадры за
боевой FPS не выдавались; новых PNG для этой nav-only правки не снималось.

## Done / remaining

| Работа | Статус |
|---|---|
| Patch10 review/rebase | done, с boundary coverage; ранее сделанное не дублировано |
| Exactness pose/battle/work budgets | done на текущем648→649 |
| Текущие focused/nav/movement/support/blasts/docs | done, skip0 |
| Фоновый боевой A/B1080/4K | done как диагностика; ordinary FPS не принят |
| Остатки623 | remaining без смены статуса: departing532мм, vertical drops и точная историческая поза; walking648-623-627.json |
| 628/625/627 | remaining:forest front/пул/фризы, масштаб/темпSIM/приказы, единый финальный source/release/delivery |

Все изменения compile inputs ограничены6 nav-файлами. UI647, walking07/08,
AMBER, physical foliage и SOLID_SAMPLES64 побайтово сохранены относительно648;
SIM/save schema/баланс/зависимости не менялись. Все незакоммиченные работы
сохранены. Release не пересобирался, «Полигон»627M не переключался. Canonical SHA
`967bdacc50aff659a9f485808edda7e0c73c0ca5eb36ddd263efd5faea989032`.
Все собственные процессы завершены перед финальным receipt; фоновой работы нет.
Итог:out/round649/final-evidence.json и ledger.json; reviewed-current.patch
содержит только текущую дельту, не весь ранее незакоммиченный WIP.
