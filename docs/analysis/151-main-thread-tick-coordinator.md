# ADR-557 — координатор тика вне общего render compute executor

04.10.2026, исходный HEAD `bf2faa4`, WBPERF-002/001/027 частично.
Артефакты: `out/fix557`, frozen build, ordinary native pairs,
`native-summary.json`, `worker-evidence.json`, `physics-job-counts.json`,
matching core/render diagnostics, отдельный GPU diagnostic, actions и test logs.
Измерения последовательные; сборки, тесты и анализ выполнялись между runs.

## Проблема и принятое изменение

Основной `advance` — длинный синхронный coordinator: authoritative step,
ожидание physics/navigation jobs, затем обновление picture inputs.
Bevy multithreaded Update executor отправлял его в общий ComputeTaskPool.
Тем же пулом пользуются pipelined render preparation и наши render jobs.
В baseline diagnostic 1800 тиков исполнялись на четырёх compute OS threads;
main thread одновременно ожидал завершения Update. Это конкуренция длинного
синхронного coordinator с render producers, а не независимая симуляция.

В tuple SystemParam функции `advance` добавлен штатный Bevy `NonSendMarker`.
Теперь coordinator исполняется на главном потоке main-world schedule.
Marker не читает ресурс и не делает систему exclusive. Chain/order, fixed
ticks, Commands, authoritative state и все остальные системы прежние.
Физика и навигация продолжают читать immutable inputs в своих существующих
постоянных пулах, результаты принимаются в прежнем порядке. Размеры pools,
render compute override, эффекты, баланс и разрешение окна не изменены.
Нет новых потоков, CPU affinity, full-world clones или frame-skipping.

Это разделение ответственности имеющихся исполнителей. Главный поток
по-прежнему ждёт step; **WBPERF-002 и цель 165 FPS этим не выполнены**.
Следующая архитектурная работа — owned sim вне frame с compact read model.

Локальный источник scheduler contract: pinned Bevy 0.19.1,
`bevy_ecs/src/system/system_param.rs::NonSendMarker`, который задаёт
`SystemStateFlags::NON_SEND`; render mesh collection использует
`bevy_pbr/src/render/mesh.rs::collect_meshes_for_gpu_building` и общий
`ComputeTaskPool::scope`. Факт конкуренции не доказывает конкретную lock
wait или то, что render thread лично исполнял coordinator: до изменения
наблюдались именно четыре compute callers, не render caller.

## Ordinary native полный бой

Четыре runs по 120 с: before→after→after→before. Сценарий 30 на 30,
AI обеих сторон, physics/nav=8, compute=4, 1600×900 AutoNoVsync,
штатные эффекты, detailed collectors выключены. Все manifest/offline
pair gates проходят; каждый run — 3600 steps при темпе 1×. Full gameplay
evidence: shots/damage/alive/fuel/streams и hash `bcc1f58c226d1721` прежние.

| Метрика | До / 1 | После / 1 | До / 2 | После / 2 |
|---|---:|---:|---:|---:|
| FPS | 81,1047 | 82,8767 | 83,5965 | 83,8471 |
| Mean frame, мс | 12,329749 | 12,066116 | 11,962227 | 11,926470 |
| p99 frame, мс | 31,6656 | 31,4424 | 30,9515 | 31,4785 |
| Max frame, мс | 67,5061 | 65,9158 | 62,8047 | 63,4427 |
| Core call/step, мс | 14,564183 | 14,425090 | 14,263709 | 14,388973 |
| Occupancy/step, мс | 1,978185 | 1,918406 | 1,970855 | 1,914549 |

Observed FPS +2,185% / +0,300%. Первый core call быстрее на 0,955%,
второй **медленнее на 0,878%**. Occupancy per-step wall ниже на
3,02% / 2,86%; query algorithm не менялся, влияние scheduling возможно,
но точная CPU/OS причинная атрибуция не выполнена. Вторые p99/max frame
хуже; небольшие различия не объявляются крупным устойчивым FPS gain
или устранением stalls. Process core-equivalents около 2,93…2,98,
не доказательство равномерной загрузки процессора.

## Фактические исполнители и render preparation

Отдельная matching diagnostic пара по 60 с, after→before:
CorePhases/CpuThreads/RenderTimeline включены, GPU timestamps выключены.
1800 measured/traced steps каждого run, physics coverage missing=0,
clock/identity mapping проходит. State/gameplay evidence совпали;
hash на 1800 ticks `bdfb97cef98d13e2`. **Эти runs не подходят для FPS
приёмки**: callbacks/OS counters/trace storage влияют на выполнение.

Before tick callers: `418396` (487), `578844` (447), `314652` (434),
`582904` (432). Main pipeline-handoff caller `403608` отличен от них.
After все **1800 ticks** исполнил `253044`, тот же OS ID, что main
pipeline-handoff callback; worker-evidence script проверяет это равенство.
Actual compute count остался четыре.

В after восемь других OS IDs выполняли parallel floor/gather:
`152940, 16664, 171092, 193768, 340820, 536636, 605952, 659732`.
Маршруты наблюдались на восьми отдельных navigation OS IDs:
`82892, 51540, 577452, 236732, 58892, 25812, 458792, 344128`.
Полные physics summary totals floor/gather в обеих версиях совпали:
88799 / 5756 jobs, включая малые serial calls. Остальные map_init
scope counts меняются с динамическим разбиением Rayon и не равны числу
фрагментов или объёму физической работы; их равенство не требуется.
Retained longest scope counts отдельно не подменяют полные job totals.

| Diagnostic render_prepare wall, мс | До | После |
|---|---:|---:|
| Mean | 3,313814 | 2,964434 |
| p99 | 21,5273 | 7,6006 |
| Max | 61,5453 | 13,2632 |

В этой паре mean −10,54%, p99 −64,69%. Это wall schedule parent,
не CPU utilization, GPU execution или только mesh collection. В него
входят parallel jobs и scheduling. Отдельные SystemMeta calls/OS stacks
этой пары не сняты, точный виновник остатка внутри prepare не установлен.
Изменение coordinator переносит также его diagnostic wrapper на main,
поэтому разницу instrumented tails нельзя объявлять производственным
FPS выигрышем. Принят явный execution boundary, проверенный native runs,
а не обещание такого же процента ускорения всех кадров.

Первый отдельный baseline diagnostic с GPU timestamps и RenderTimeline
сохранён в `before-diagnostic-60`. Он показал expensive render_prepare
вне gas leaf и отдельные GPU pass distributions. Его flags отличаются
от matching пары; он не используется для speed comparison. Percentiles
GPU passes не складываются, source-frame GPU attribution отсутствует.

## Оставшийся критический путь и проверки

В matching diagnostics общий дорогой **тик 1606**, показанный в следующем
кадре с tick=1607: traced wall 52,560 / 52,896 мс. Два крупных повторения
`orders_misc` занимают 10,524 + 9,475 мс до и 10,507 + 9,807 мс после.
Эти интервалы требуют дальнейшей attribution к конкретному weapon/order
call, не объясняются самим названием phase. Пик не устранён переносом
coordinator. Не приписывать preceding core текущему tick row.

- Full release `render --bin blast`: **118 passed, 3 прежних ignored**,
  112,83 с. Новый тест проверяет initialized scheduler NON_SEND и отсутствие
  EXCLUSIVE; существующие battle/worker-rebuild/UI/lifecycle tests проходят.
- Clippy release bin/tests проходит; новые строки не добавили warnings,
  прежние dependency/render/test warnings сохранены.
- Own Rustfmt и diff checks проходят. Sim-core алгоритмы не изменялись;
  прошлый final baseline suite ADR-556: 561 passed / 4 ignored.
- Native actions 30 с: generations 1…4, 755 steps, intended pause/edit
  transitions, evidence gates проходят. Это отдельная проверка actions,
  не full-battle FPS comparison. Preview/видео не снимались.

After source inputs прошли ADR-554 refresh/hash/freeze protocol.
Before reused verified ADR-556 fresh control, исходный blast совпал
побайтно с HEAD; его listed burst hash тоже совпадает. Это ограниченная
аттестация перечисленных inputs, не всей dependency closure/clean checkout.
Все прежние 13 WIP entries сохранены вне нового commit.

- Before executable SHA:
  `3c1ed8b7f9513ffba14d5aea7b598a858151481a8061e6abef24b7682acbe03c`.
- After/current `target/release/blast.exe`:
  `f59b60a22f17fc3fa08824c63aeb2be0e49ce3545ab42a6c85f8f398e9f4016a`.
- After blast source:
  `7868dedfb754f306235206c70c18d0b0cb78682054eb41561f47d3a476ce4b38`.

WBPERF-002 остаётся главным architecture пунктом. WBPERF-001/027 дальше:
specific calls внутри orders_misc тика 1606, changed mesh queues и actual
render jobs/OS scheduling. Rare stalls, независимый sim/render, равномерная
загрузка CPU и 165 FPS открыты.
