# Короткий sweep prefix и отклонённые geometry optimizations

04.10.2026, ADR-559. Исходный main: `35679f7`. Полный world battle,
60 units / AI обеих сторон, штатные эффекты. Каталог evidence: `out/fix559`.
Исходные WIP в sim/weapon/UI/data/docs присутствуют в обеих версиях;
это сравнение текущей рабочей сцены, не чистого checkout.

## Принято: не считать отброшенную геометрию префикса

Fresh direct connection и продолжение поиска рассчитывали первые четыре
pose целиком, затем проверяли `prefix_open`. При отказе на первой/второй
pose оставшиеся ответы только отбрасывались. `Probe::route_prefix` проверяет
ordered prefix после каждого ответа и прекращает подготовку при отказе.
Initial-pose exception и exact height/clearance/step semantics остаются
у существующего predicate. Перед workers остаётся тот же последовательный
префикс; после допустимых четырёх poses работает прежний indexed fan-out.

Новый helper применяется к fresh и suspended sweeps. Logical node/pose
allowance, commit order и hash не зависят от числа speculative reads.
Это уменьшение ненужной подготовки, не новая схема параллелизации.
Размеры и количество pools не менялись; balanced CPU не измерен.

Два новых теста сравнивают все pose поля с eager geometry, включая initial
exception, пустой запрос и полностью допустимый prefix. При раннем отказе
считается одна/две позиции вместо четырёх; `samples` и `columns` меньше.
Это проверенный локальный результат, не обещание такого же сокращения
полного поиска: кэши и дальнейшие маршруты имеют собственную стоимость.

## Headless A/B и повтор

1800 ticks, physics/nav=8. Порядок control→final→final→control.
Полные финальные counters и hash `bdfb97cef98d13e2` совпадают.
Все величины ниже — wall timings CPU harness, не GPU/native FPS.

| Run | Mean tick, мс | p99 tick, мс | Max tick, мс | Mean routes, мс |
|---|---:|---:|---:|---:|
| Control / 1 | 10.317982 | 17.967 | 37.064 | 2.149945 |
| Final / 1 | 10.293787 | 17.542 | 35.421 | 2.152340 |
| Final / 2 | 10.348068 | 18.014 | 44.397 | 2.141837 |
| Control / 2 | 10.362001 | 17.718 | 36.391 | 2.160534 |

Mean tick −0,234% / −0,134%: слишком мало для вывода об устойчивом gain.
`orders_retarget_prepare` mean 117→106 мкс, повтор 105→110 мкс;
p99/max 4,299→3,872 мс, повтор 4,018→4,080 мс. Остаток порядка 4 мс
не решён. Tick 1024 во втором final: 44,397 мс, из них rubble 27,042 мс.
Пик сохранён, OS wait/стек/причина по этой границе не установлены;
его нельзя объявлять исправленным или отбрасывать из сравнения.

## Ordinary native: четыре runs по 120 секунд

Порядок before→after→after→before; 1600×900, AutoNoVsync, compute=4,
physics/nav=8, detailed collectors/GPU timestamps выключены.
Все четыре evidence gates проходят; каждый run 3600 steps, tempo 1×.
Полные gameplay PROFILE lines (shot/damage/alive/streams/fuel) и hash
`bcc1f58c226d1721` совпадают. Анализатор запущен после завершения captures.

| Run | FPS | Mean frame, мс | p99 frame, мс | Max frame, мс | Core call/step, мс |
|---|---:|---:|---:|---:|---:|
| Before / 1 | 85.097127 | 11.751278 | 30.8061 | 61.1450 | 14.188914 |
| After / 1 | 85.261104 | 11.728678 | 30.5920 | 59.8435 | 14.219730 |
| After / 2 | 84.072057 | 11.894559 | 30.6451 | 63.8886 | 14.240516 |
| Before / 2 | 83.945479 | 11.912494 | 30.8672 | 61.2062 | 14.295101 |

FPS +0,193% / +0,151%; core/step +0,217% / −0,382%. Это практически
прежний общий FPS, не существенное ускорение. p99 немного ниже в обеих
парах, но второй max хуже. Принято локальное удаление discarded geometry
с малым изменением кода и сохранением поведения; общий FPS/hitch fix
не заявляется. 165 FPS и редкие многосекундные stalls остаются открыты.

## Отвергнутые кандидаты

Отдельные frozen binaries/source copies/build manifests/CPU CSV сохранены.
Контрольные mean ticks 10,315 / 10,333 мс; final summaries/hash одинаковы
во всех семи exploratory runs. Эти кандидаты не прошли до native acceptance.

| Candidate | Mean tick, мс | Результат |
|---|---:|---|
| Const-generic crush/no-crush specialization | 10.357 | Один CPU A/B, выигрыша нет; откат |
| Early threshold proofs для World и Rubble | 10.393 / 10.494 | Два CPU runs, выигрыша нет; откат |
| Dense debris tile index + exact touches shortcut | 10.443 / 10.427 | Rebuild rubble 523–530→602–606 мкс; route/motion mean почти прежние; откат |

Tile-upper-only executable собран, отдельно не измерялся; его результат
не приписывается combined index. Индекс не входил в final. Threshold APIs
и const specialization удалены, World восстановлен байт-в-байт. Частоты
успеха proofs не измерялись; отсутствие общего gain не доказывает редкость
их попадания. Для следующей такой гипотезы нужны hit/work counters.

## Проверки и provenance

- Full release nav: 106 passed; sim-core: 562 passed / 4 прежних ignored.
  Existing matrices покрывают serial/parallel route state, live blockers,
  terrain edits, worker-count hashes и replay. Два новых nav tests проходят.
- Clippy release nav/sim-core lib/bins/tests проходит с прежними warnings.
  Formatting собственных строк и diff checks проходят.
- Release range собран и проверен четырьмя ordinary native runs.
  UI/render test suite отдельно в этом шаге не запускалась; UI не менялся.
- Source refresh/freeze protocol ADR-554 применён ко всем восстановленным
  source inputs. Hashes текущих listed inputs совпадают с final native build
  manifest. Это ограниченная source attestation, не полная dependency closure.
- Bench final SHA256: `35654e2d5e151b02235a4b29f57505029aac9f2ff5e5d747b8992e3ab81a4939`.
- Native final/current release SHA256:
  `28cf03be2b7c548115ab9b7458fa310e6b3ef3a00f70b609730bed4dd344596b`.
- Native before SHA256:
  `c7349e72bacb55b14d1c4a14ec66234db4ba0f8b56e204d5c1fbfff64b9e3015`.
- Evidence: prototype-summary.json, final-cpu-summary.json,
  native-summary.json, *.build.json, CSV/logs и четыре analysis reports.

Следующий приоритет — WBPERF-002, независимая симуляция и компактный read
model, затем WBPERF-001/027: actor/job attribution оставшейся подготовки,
cache/proof work и OS stacks/waits для пиков. Увеличивать число маленьких
jobs без измеримого выигрыша нельзя. Этот шаг не закрывает CPU balance,
165 FPS, общий critical path или остаток direct sweep preparation.
