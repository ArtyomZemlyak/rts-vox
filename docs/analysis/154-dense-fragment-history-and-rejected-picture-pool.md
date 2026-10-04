# ADR-560 — плотная история поз в параллельной подготовке обломков

04.10.2026. Родитель `9a57a4d`, рабочая копия содержит прежние 13 WIP entries.
Изменение только в render; физика, баланс, лимиты эффектов и размеры pools прежние.

## Что изменено

До шага `moving::before` строил ID→position hash table для всех физических
обломков, включая resting. Каждый render leaf повторял hash lookup на каждый
обломок. Теперь reusable Vec хранит `(ID, position)` в физическом порядке.
Строго возрастающая история не строит map. Каждый прежний пакет по 2048 кусков
начинает с `partition_point` и сливает возрастающие IDs в своём worker job.
Пакет проверяет собственный порядок; reordered current input использует lookup,
а unordered/duplicate previous input — compatibility map с прежним last-write-wins.
Новая геометрия использует current position при отсутствии ID.

Сохраняются resting/wake origins, alpha/previous interpolation, pause/reset,
ID reuse, внешний despawn, Stamp, mesh keys и физический порядок ECS/Assets
publication. Captured input immutable, slots принадлежат своим leaves; scope
и существующий shared ComputeTaskPool прежние. Полный World не копируется.

## Ordinary native A/B

Все четыре прогона по 120 с последовательны: before → after → after → before.
60 юнитов AI-vs-AI, stock tuning, 1600×900, AutoNoVsync, physics=8,
navigation=8, compute=4, эффекты нормальные, без core/render/task collectors.
Во время timings нет сборок, тестов или анализа других прогонов.

| Run | FPS по begin intervals | Mean frame, ms | p99, ms | Max, ms | Core/step, ms | Mean fragments/frame, ms | Fragment p99, ms |
|---|---:|---:|---:|---:|---:|---:|---:|
| before 1 | 83.962 | 11.9101 | 30.8769 | 50.7819 | 14.27969 | 0.80602 | 2.3599 |
| after 1 | 84.469 | 11.8386 | 30.5132 | 59.6448 | 14.15887 | 0.75268 | 2.2653 |
| after 2 | 84.589 | 11.8218 | 30.4899 | 52.6843 | 14.17034 | 0.74995 | 2.2587 |
| before 2 | 84.549 | 11.8275 | 30.6268 | 49.8956 | 14.26504 | 0.79306 | 2.3649 |

Сравнения eligible без исключения ключей. Каждый run acceptance-eligible,
3600 actual steps, tempo 1×, hash `bcc1f58c226d1721`; полные PROFILE fuel,
stream/damage/melting и battle shots/damage/alive lines совпадают во всех четырёх.
Mean fragment phase −6.62% / −5.44%, p99 −4.01% / −4.49%.
FPS +0.604% / +0.048%; существенного общего gain не доказано.
Core/step −0.846% / −0.664% не означает ускорения физики: её код не менялся.
Оба max frame хуже, первый core max вырос 38.4→49.1 мс; пик не скрывается
и его источник этим изменением не объяснён. Mean occupancy немного хуже
0.690→0.722 / 0.687→0.721 мс; packed algorithm и входы не менялись.
Принято ограниченное ускорение fragment preparation, не решение tail/FPS.

## Отклонённый отдельный picture pool

Сначала immutable occupancy/fragment jobs переключались на persistent
отдельный Bevy TaskPool через экспериментальный CLI; pipeline/physics/nav
оставались прежними. В одном frozen binary проверены shared → 2 → 4 → shared,
каждый по 120 с. Runtime counts и CLI подтверждены; captures и тесты gates
сохранены в `out/fix560/rejected-picture/`, изменения полностью откатились.

| Picture workers | FPS | Mean frame, ms | p99, ms | Core/step, ms |
|---|---:|---:|---:|---:|
| shared first | 71.238 | 14.0375 | 31.4728 | 14.49696 |
| 2 | 67.144 | 14.8933 | 32.6286 | 14.44623 |
| 4 | 71.589 | 13.9686 | 31.4761 | 14.45273 |
| shared last | 72.227 | 13.8452 | 31.0615 | 14.40957 |

Два потока также выбирают прежний concatenation/radix occupancy path вместо
packed path от четырёх threads: это confound, а не доказательство причины
замедления. Четыре не выигрывают относительно обоих controls. Между этой
серией и поздним A/B заметно различается FPS; причина не установлена,
сравнивать 71 и 84 как gain новой истории нельзя. Build variants и контекст
сохранены, отсутствие известных competing processes не доказывает отсутствие
внешней GPU/OS нагрузки или влияния executable scheduling/codegen.
Новых pool/resource/CLI/profiler/tool gates в конечном коде нет.

## Проверки и воспроизведение

`cargo-local.ps1 test --release -p render --bin blast -- --test-threads=1`:
119 passed, 3 прежних ignored. Новый history differential test сравнивает
lookup/cursors с map при compaction, spawn, reorder, duplicates, empty/reset
и разных leaf sizes. Прежние parallel/serial tests, реальные UI/battle,
resting/wake, отсутствующие entities и интерполяция проходят.
Clippy release render bin/tests проходит с прежними unrelated warnings;
новые explicit-auto-deref замечания в тесте устранены. Format и diff checks.
После test-only cleanup повторён реальный before/reset history test.

Frozen after: `out/fix560/dense-history.exe`, SHA256
`BF576F24DA0EE6CC20360BA73E9AC71C0A82F0D041DF919E4157A317A3A75D7C`.
Before: `out/fix559/prefix.exe`, SHA256
`28CF03BE2B7C548115AB9B7458FA310E6B3EF3A00F70B609730BED4DD344596B`.
Rejected executable: `out/fix560/picture.exe`, SHA256
`4836056FAE3036669B45DBEA4451D1337E51677410661A95EC336820D6A737FE`.
Build manifests attest listed refreshed source hashes, не полный clean closure.

Пример обычного запуска (новый Output; не перезаписывать сохранённые runs):

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/profile-range.ps1 `
  -SkipBuild -BinaryPath out/fix560/dense-history.exe -Scenarios world-battle `
  -Seconds 120 -Output out/fix560/recheck -PhysicsWorkers 8 `
  -NavigationWorkers 8 -ComputeWorkers 4 -TargetFps 165
C:/Python314/python.exe -X utf8 scripts/analyze-range-profile.py out/fix560/recheck --require-valid
```

Все raw CSV/log/err/manifests/OS process observations и analysis JSON сохранены
в `out/fix560/{before-native-120,after-native-120,after-native-repeat-120,before-native-repeat-120}`;
`final-native-summary.json` содержит агрегаты. Screening —
`{shared-first,picture-two,picture-four,shared-last}`, `screening-summary.json`.

WBPERF-002 остаётся главным архитектурным пределом: кадр синхронно ждёт
~14 мс tick. Прямых чтений Scene.sim более 400; перенос step на worker с
same-frame join сохраняет этот предел. Нужен отдельно владеющий sim worker,
команды с tick/generation/sequence и bounded completed render publication,
без полного World clone, пропуска hit/death/terrain событий и замедления 30 Hz.
WBPERF-027, CPU balance, редкие пики и 165 FPS остаются открыты.
