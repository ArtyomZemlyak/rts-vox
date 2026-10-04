# ADR-566: лишние входы локальных поисков и контекст сборок

## Принятый код

Параллельная фаза маршрутов собирала `Vec<Obstacle>` из соседей для каждого
слота, хотя `prepare_local_hunt_observed` сразу отказывался от работы для
неактивного поиска. Теперь общая проверка `needs_local_hunt` стоит перед
сборкой списка и внутри потребителя. Проверка выполняется **после** terrain
continuation: этот проход может уже завершить поиск. Снимок соседей, порядок
live commit, callbacks, allowances, pools и формат сохранения прежние.

Это удаление лишней подготовки в существующих потоках, а не новый executor
или доказательство равномерной загрузки CPU.

## Что отклонено

Длинные local spider sweeps пробовались на том же immutable terrain input
через существующий nav pool. Clearance callbacks оставались последовательными;
первые четыре позиции проверялись перед спекулятивной геометрией. Тест
сравнивал callbacks и сохранённый поиск на 1/2/4/8 потоках, реально выполняя
дочерние задачи; 46 motion tests прошли.

Четыре дочерних batch: CPU motion −4,68/4,51%, tick −2,35/2,13%, но native
FPS −1,35/1,85%. Второй max frame 49,21→87,33мс. Два batch: CPU motion
−2,61%, total практически прежний; native FPS −0,62%. **Оба варианта
удалены**, включая их новые tests/observer paths. Плохие результаты сохранены.

## Проверяемость сборок

Прежний frozen manifest записывал только явно обновлённые sources. Он не
фиксировал compiler/profile context и не гарантировал пересборку зависимостей
после восстановления экспериментального файла с прежним timestamp.

Builder теперь записывает `rustc -vV`, `cargo -V`, заданный перечень compiler/
release flags и hash `cargo-local.ps1`. `-RefreshTrackedInputs` обновляет
timestamps и проверяет hashes всего отслеживаемого набора Rust/WGSL/RON/
Cargo/config/toolchain inputs вместе с явно заданными sources. В этой паре
340 inputs; отличаются только `blast.rs` и `range_motion.rs`. Rust 1.98.1.
Manifest schema 1 сохранена; новые поля добавлены совместимо.

Analyzer сравнивает записанный context, включая refresh mode. Несовпадение
или наличие context только с одной стороны отклоняет pair. Два legacy captures
сохраняют прежние gates, но получают явный `legacy_unrecorded`; это не
подтверждение равного контекста сборок. Совпадение новых полей помечается
`matched_recorded`, **не полной воспроизводимостью всей цепочки**: generated/
untracked inputs, SDK, registry contents и linker outputs не аттестованы.

Исторический baseline SHA `E59DEA…` и свежий `398B6C…` отличаются. Исходные
WIP sources совпали по bytes; разницу бинарников не объявляем причиной FPS
или доказательством stale code. Финальное решение использует свежую пару,
пересобранную с одинаковым context и обновлением зависимостей.

## Результаты финального варианта

Stock full-map AI-vs-AI, 30×30, 120с, 1600×900, AutoNoVsync,
physics/nav/compute 8/8/4. Timed captures выполнялись последовательно;
сборок, tests и тяжёлого анализа одновременно с ними не было.

| Свежая пара | Контроль → фикс | Core/step | Frame p99 | Max frame |
|---|---:|---:|---:|---:|
| первая | 91,39→91,43 FPS | 13,433→13,386мс | 29,221→28,914мс | 50,353→50,444мс |
| повтор, запуск фикса первым | 89,42→89,57 FPS | 13,442→13,392мс | 28,900→29,004мс | 50,315→51,671мс |

Изменения FPS +0,037/+0,161%, core −0,353/0,375%. **Значимый FPS gain не
доказан**; max не улучшился. Дрейф контроля между парами больше результата.
Сборки: baseline `398B6C05…`, фикс `43D5FB13…`, полные hashes в manifests.

CPU exploratory пары с прежним frozen baseline: motion −0,67/1,15%,
tick +1,45/−0,38%; устойчивый общий gain также не установлен. Каждый из
3600 tick counters, полные gameplay/unit lines и hash совпали. CPU результаты
не подменяют native acceptance и не имеют нового complete tracked context.

Во всех обычных native captures 3600 ticks, 1×, четыре полные gameplay
строки совпали, hash `bcc1f58c226d1721`. Legacy результаты, включая просадки
финального варианта относительно старого executable, не удалены.

## Камера и потоки

Отдельный diagnostic tour финального свежего executable: CorePhases,
CpuThreads, OsPressure, 120с. 21 segments, 145 window origins, 0 blocked
frames; 3600 traced ticks, tempo ratio 0,999590, strict tempo true, тот же
gameplay/hash. Frame p99 34,410мс, max 51,629мс; streaming max 19,507мс,
sampled maximum no-tick interval 33,125мс. Diagnostic не квалифицирует FPS.

Observed route OS threads — 8, translation OS threads — 8. Это coverage
работы существующих pools, не доказательство balanced CPU. Многосекундные
freezes здесь не повторились и не объявляются исправленными.

## Проверки и следующий приоритет

Финальная source: 45 motion tests, golden 4/save 19, fmt проходят.
Обычный Clippy проходит с прежними warnings; `-D warnings` остановился на
прежнем `type_complexity` в `voxel/world.rs`. 124 analyzer tests и PowerShell
validity/parser/build-evidence suite проходят. Исходные 13 WIP сохранены;
в mixed files индекс содержит только текущие изменения. Установлен свежий
финальный native binary.

WBPERF-003/004: убрать ненужную подготовку; WBPERF-001/027: записанный build
context и честные limits его сравнения. WBPERF-002 остаётся главным:
main синхронно ждёт tick. Также открыты dispatch/join/OS attribution,
streaming tails, rare stalls и 165 FPS. Увеличивать число workers без
native A/B и проверки actual work недостаточно.

Raw, rejected sources, binaries/build manifests, tests, counters и отчёты:
`out/fix566/`. Итоги: `core-summary.json`, `native-summary.json` и
`fresh-filter-camera-120/world-battle.analysis.json`. Raw не входит в commit.
