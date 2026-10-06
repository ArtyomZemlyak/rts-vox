# ADR-597 — скрытые этапы карты обломков

Дата:2026-10-06. Продолжение ADR-596.

## Диагноз и итоговый фикс

Прежний native out/fix596/camera-core:tick1689 parent rubble wall10.481мс,
longest columns0.611мс/floor0.059мс/gather0.027мс. Floor около0.22–0.41мс,
columns begin9.457мс. Не были разделены classify dispatch/join, support
index build/query и release. Поэтому приписать всё время ожиданию пула
или ОС было бы недоказанным выводом. Фикс устраняет этот пробел наблюдений.

Новые парные physics_rubble_* scopes:prepare,classify,support_build,
support_query,support_drop,gather_dispatch,sort,dedup. Classify/gather
включают scheduling, join и ordered merge; это wall родителя, не чистый
wait. Прежние floor/gather/columns и boxes/queries/candidates/cells
сохранены. Scopes вложенные, не складываются с родителями как CPU.
Часы/статистику хранит внешний observer. Обычные no-op callbacks не
влияют на решения/состояние/материю/caps/порядок или snapshots.

## Повторный эксперимент и отказ от него

Кандидат распределял1024+ stacked queries по indexed chunks256 с прежним
ordered join. Quiet world60/1800 сохранял hash1f697f865e6f8aa6, все12окон
каждой damage-метки,143 wave_progress и48 fire_window. Working lib655
passed,4manual ignored; index5rubble +4golden +19save passed. Frozen trial
out/fix597/after.exe SHA0286E7C9F8ACAC836CFEF7DD78B56A896AC7E0489E34007F7897B9A567DA330E.

| CPU, мкс | Before | Trial |
|---|---:|---:|
| rubble mean | 1102 | 1106 |
| rubble p95 | 1654 | 1655 |
| rubble max | 2071 | 3433 |
| tick mean | 10303 | 10251 |
| tick p99 | 15694 | 15529 |
| tick max | 20670 | 30206 |

Выигрыш не доказан; maximum trial связан с debris22.086мс. История
ADR-565/analysis159 уже содержит такой же rejected candidate с двумя
CPU парами. Повтор подтверждает прежнее решение: parallel support и
дополнительные output buffers откатились. Итоговый код сохраняет исходный
последовательный support iterator, добавляя только observer boundaries.
Trial3600 hash ebbee0fe0becc090 сохранился; его timings не квалифицируют
ускорение — он шёл одновременно со сборкой индекса.

## Квалификация итогового варианта

Все результаты — out/fix597/. Исходные WIP удержаны вне коммита;
working binaries сохраняют этот состав, index export проверяется отдельно.
Финальный scope test использует nonzero Bursts hash, реальные supported
и floating pieces, workers1/4/8,1023/1024/1025 candidates. Проверяет
reference карту, закрытие всех scopes и ровно одну query scope, независимо
от числа workers. Final index export — isolated-final.

Изолированный итоговый index:651 lib passed,4manual ignored;4golden и
19save passed. CLI seed42/ticks1000 сохраняет66041a9f60189841.
Staged Rust byte-identical tested export и рабочему коду. Свежий native
out/fix597/native.exe SHA06FF57E98F9B6F06FC13718E496F81A4F3899F84F7C48E7790DA8022824734CF;
все355 заявленных Rust/WGSL/RON/Cargo inputs неизменны через сборку и
после неё. Это working-состав с исходными WIP, не clean index binary;
SDK/registry/generated inputs manifest не аттестует.

## Полный native профиль с камерой

Команда:profile-range.ps1 -Seconds60 -Scenarios world-battle -SkipBuild
-BinaryPath out/fix597/native.exe -Gpu -CameraTour -TargetFps165
-NavigationWorkers8 -PhysicsWorkers8 -CorePhases -RenderTimeline
-CpuThreads -OsPressure. Данные — out/fix597/camera-core; анализ —
rubble-scopes.json и analyze_scopes.py в out/fix597. Сборки/тесты до
capture завершены. ValidForComparison=true, AcceptanceEligible=false:
подробные observers/CPU/OS pressure имеют overhead и не квалифицируют FPS.

Все1800 ticks1..1800, realtime ratio0.9992; тот же hash1f697f865e6f8aa6
и итоговые damage/shots/alive, что ADR-596 diagnostic. Render spans77324,
0dropped/0incomplete. Камера:11segments,104origins,0blocked,
stream max15.587мс. Mean99.1FPS,p99 28.933мс,max41.164мс;
165FPS не достигнуты. Старые10.481мс rubble/59мс interval не повторились;
это не доказательство их исправления.

| Новая rubble scope, wall мс | mean | p99 | max |
|---|---:|---:|---:|
| classify dispatch/join | 0.219 | 0.419 | 0.510 |
| support build | 0.501 | 0.947 | 1.445 |
| support query | 0.030 | 0.054 | 0.152 |
| support drop | 0.033 | 0.066 | 0.209 |
| gather dispatch/join | 0.106 | 0.275 | 0.629 |
| sort | 0.239 | 0.463 | 0.577 |
| dedup | 0.042 | 0.093 | 0.219 |

Support scopes имеют1767samples (stacked бывают не на каждом тике),
остальные1800. Это родительские wall intervals, не CPU utilization.
В самом длинном parent rubble tick1698:3.261мс; prepare0.317,
classify0.300,build1.252,query0.038,drop0.050,columns0.859мс.
Columns включает gather0.312/sort0.463/dedup0.077; их повторно не суммируем.
Caller cycles7135850 отдельно, в миллисекунды не переводим.
Следующий кандидат в этой фазе — построение индекса, а не распараллеливание
дешёвых queries. Самый долгий join classify в этом повторе всего0.510мс;
прежняя причина10мс остаётся не доказанной без воспроизведения.

Frame max41.164мс на39.671с: предыдущий update33.074мс +gap8.090мс.
Предыдущий core13.266мс, в соответствующем tick1189 motion5.791мс,
debris2.852мс,rubble1.454мс. Текущие поля следующего кадра не объясняют
предыдущий interval; gap не обозначает чистый idle или ожидание пула.
Реальные приоритеты: WBPERF-002 synchronous tick/render publication,
frame handoff/present/stream tails и motion/debris; для rubble — build.
Полный CorePhases нужен для поздних пауз: ограниченные4 trigger windows
заканчивались рано.165FPS, rare stalls и physical tails P-87 открыты.
