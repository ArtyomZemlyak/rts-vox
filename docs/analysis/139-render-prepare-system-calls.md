# Actual Bevy system calls в подготовке рендера — ADR-545

## Что мешало понимать задержки

ADR-543 устранил накопление terrain journal; ADR-544 сохранил поздние
кадры длинного профиля. В ADR-544 оставался prepare 32,127 мс при actual
gas 0,046 мс. Driver CPU не охватывал другие render workers, а родительский
wall span не называл выполнявшуюся систему. Это не доказывало GPU wait,
mutex wait или нехватку потоков. Увеличивать пулы по этим цифрам было рано.

Теперь optional `render-tasks` использует существующие cached SystemMeta
spans Bevy 0.19.1: `system` и `system_commands`. Layer устанавливается через
LogPlugin **до создания систем**. Вызов наблюдается на фактическом потоке,
включая parameter access и тело функции; commands обозначаются отдельно.
Расписание, физика, пулы, cadence и качество картинки не менялись. Нет
unsafe замены систем или новых exclusive fences между jobs.

Включены существующие Bevy `detailed_trace` и `debug` для реальных имён.
Новых crate/version нет; Cargo.lock добавляет только dependency edge от
bevy_ecs к уже существующему tracing. Обычная сборка не включает feature,
observer и буфер; feature build исключается из FPS-приёмки даже без сбора.

## Источник, полнота и цена наблюдения

- Имена Render/Extract schedules фиксируются после построения plugins,
  до первого extraction. Совпадающие имена main/render и собственные
  probes исключены, список exclusions сохраняется. Это не поддержка
  динамического добавления plugins во время замера.
- Prepare scope публикует уже замороженный source row/generation/tick.
  Каждая запись владеет этой меткой; end не читает следующий main frame.
  Записываются wall, Rust/OS thread, доступные thread CPU и cycles.
- Буфер ограничен 2000000 записями. В native сборке запись 208 bytes:
  резерв элементов 416000000 bytes / 396,73 MiB, **не RSS**. Для 120 с
  coarse timeline дополнительно резервирует 201,62 MiB. Запись файлов
  идёт после cutoff; overflow и terminal open остаются явными.
- До 20 секунд снимаются все source frames, длиннее — каждый восьмой
  целиком. Metadata/report сохраняют stride; parser проверяет кратность
  row и сравнивает режим выборки. Неснятые кадры, в том числе редкие
  stalls в них, не получают task attribution. Это не полнота всех jobs
  длинного боя и не адаптивный stall-trigger.
- Общая длительность наблюдённых задач — union wall intervals выбранного
  кадра. Вложенные commands и параллельные системы не складываются с
  родителем. `other_or_unobserved` остаётся неизвестным scheduling/work,
  а не установленным ожиданием.
- Callback, mutex recorder, tracing и OS counters добавляют нагрузку и
  могут менять scheduling. Внутренние taskpool jobs функции, условия
  выполнения и работа без SystemMeta здесь отдельно не измерены.
  Округлённый CPU может быть 0 или 15,625 мс при более коротком wall;
  cycles не переводятся в CPU milliseconds. Union — не CPU utilization.

Нативный первый запуск обнаружил ещё одну проблему наблюдаемости:
унаследованный `RUST_LOG=warn` выключил INFO SystemMeta spans. Inventory
352 имён был заполнен, actual calls — 0. Raw сохранён в
`out/fix545/battle-tasks-120`; оба gate отклонили пустую трассу. Это
не свидетельство отсутствия работы. Runner теперь сохраняет остальные
directives, добавляет `bevy_ecs::system::function_system=info`, записывает
effective RUST_LOG в manifest и восстанавливает окружение. Для прямого
CLI этот target надо разрешить самостоятельно. Пустая или обрезанная
трасса по-прежнему не проходит evidence gate.

## Воспроизведение

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/cargo-local.ps1 build --locked --release -p render --bin blast --features render-tasks
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/profile-range.ps1 -Scenarios world-battle -Seconds 120 -TargetFps 165 -SkipBuild -RenderTimeline -RenderTasks -CpuThreads
```

Для frozen exe дополнительно передать `-BinaryPath PATH`; output должен
быть новым. `.tasks.csv` и `.task-names.csv` дополняют `.render.csv`.
Offline analyzer `--require-valid` проверяет источник, clock/nesting,
inventory, thread counters, counts, stride, cutoff и capture completeness.
Feature/mode/filter/stride различия не проходят A/B. Старые profiles
без task mode продолжают читаться; отсутствие новых полей не выдумывается.

## Проверки и нативные результаты

136 library / 101 blast / 92 Python / 45 PowerShell = 374 проверки,
четыре manual tests ignored. Дополнительно обычные library/blast tests,
release build и clippy проверяют путь без feature. Новых предупреждений
нет. Реальные FunctionSystem тесты подтверждают имена и пересекающиеся
вызовы на разных потоках. Проверены reset/cutoff, bounded overflow,
whole-frame stride, contradictory identities/counters, quoted CSV names,
несопоставимые modes и включение task target при унаследованном warn. Прежний headless
hash 1000 тиков / 5000 units: `66041a9f60189841`.

Запуски последовательны, без builds/tests/правок/видео во время native
измерения. RTX 5090/Vulkan, 1600×900, stock tuning, physics/navigation
8/8; фактический Bevy compute pool 4. Frozen sampled exe
`out/fix545/sampled-tasks.exe`, SHA256
`af6d9293c2686ea23feb88ceea3166820bf75d417dfb561d103a856cafc5f5f5`.
В нём та же отдельная WIP ADR-523, не чистый Git snapshot;
SourceAtLaunch не доказывает build source. Первый all-frame exe,
сохранённый для пустой трассы и smoke: SHA256
`9fbc01b5a123a943a23097bec143b01362384db129fd6e36d824708e762d722a`.

После captures упрощена cfg-ветка сохранения sidecars, чтобы обычная
сборка не получала новый clippy warning; содержимое сохраняемых данных
прежнее. Оба feature paths повторно проходят clippy. Финальный ordinary
exe восстановлен в `target/release/blast.exe`; frozen копия
`out/fix545/ordinary.exe`, SHA256
`a4795b1d46466520a95d6ffd68990b1fcd6f381aeaf9aa3a3e9474983636403a`.
Пятиисекундный smoke проходит native/offline gate, печатает
`render_tasks_build=false`, task sidecars отсутствуют. Это проверка
обычного пути сохранения и запуска, не FPS A/B или длительная приёмка.

| Run | Main rows | Task records | Sampled source rows | Intentionally unsampled prepare rows | Task dropped/open | Offline gate |
|---|---:|---:|---:|---:|---:|---|
| `out/fix545/task-smoke-5`, stride 1 | 815 | 297924 | 814 | 0 | 0 / 0 | pass |
| `out/fix545/battle-sampled-120`, stride 8 | 8615 | 394182 | 1077 | 7537 | 0 / 0 | pass |
| `out/fix545/actions-sampled-30`, stride 8 | 10192 | 466284 | 1274 | 8917 | 0 / 0 | pass |

В обоих sampled runs исполнялись 278 из 352 разрешённых имён, пять
собственных probes исключены. Все выбранные prepare rows имеют calls.
Coarse battle: 8613 complete source rows, три terminal open spans,
одна ещё не handed-off main row. Actions: поколения 1…4, 10190 complete
source rows, два terminal open, одна ещё не handed-off main row. Они
не превращаются в полные task frames. Reset/source/cutoff gates проходят.

Длинный бой: 3600 шагов / 0,9996×, hash `bcc1f58c226d1721`, observed
mean 13,933 мс (~71,77 FPS), p99 34,197 / max 65,272 мс. Actions:
mean 2,943 / p99 6,311 / max 43,968 мс; это scripted workload с reset,
не realtime battle acceptance. Эти FPS — диагностические, **не A/B**
с ADR-544 и не приёмка 165 FPS.

| Selected source / tick | Parent prepare, ms | Task wall union, ms | Other/unobserved, ms | Longest actual call, ms |
|---|---:|---:|---:|---:|
| 7008 / 2432 | 23,7412 | 23,1990 | 0,5422 | collect_meshes_for_gpu_building: 20,7398 |
| 8464 / 3510 | 23,6225 | 23,0723 | 0,5502 | collect_meshes_for_gpu_building: 20,7475 |
| 7224 / 2590 | 23,0849 | 22,5929 | 0,4920 | collect_meshes_for_gpu_building: 20,4211 |

Полное имя: `bevy_pbr::render::mesh::collect_meshes_for_gpu_building`.
Row 7008 выполнялся на OS thread 533864; calling-thread CPU округлён до
0 мс, cycles 48704380. Row 7224: CPU 15,625 мс / cycles 51081420.
Это фактический system parent на compute worker, не GPU execution.
Исходник pinned Bevy (mesh.rs, функция начинается со строки 2407)
содержит nested ComputeTaskPool scope, producers и mpsc collection.
Нельзя приписывать весь parent wall одному CPU или объявлять его mutex/GPU
wait без child spans/OS evidence. Самый медленный coarse frame может быть
неснятым выборкой: максимум всех stalls этим рейтингом не покрыт.

В actions самый дорогой выбранный prepare: row 1456 / generation 3 /
tick 28, 7,521 мс. Mesh allocator 3,9947 мс, shadow specialization
1,1674 мс. Это отдельный путь reload, не та же причина длинного боя.

## Следующий порядок

1. WBPERF-001/027: детализация mesh instance queues — changed/new/removed
   workload, реальные producer/consumer child calls и source-aligned OS
   scheduling/stacks. Проверить лишнюю инвалидизацию meshes/transforms,
   прежде чем менять compute pool или путь GPU preprocessing.
2. Независимо проверять allocator/reload и main serial critical path;
   выбранные кадры не объясняют неснятый максимум или редкие секунды фриза.
3. WBPERF-002: независимый sim/render tick и bounded snapshots;
   GPU timestamps/fences требуют тех же source IDs.
4. После конкретного изменения — обычная сборка, fixed-workload native
   пары, реальный темп симуляции и tail latency. 165 FPS и редкие фризы
   остаются открытыми.
