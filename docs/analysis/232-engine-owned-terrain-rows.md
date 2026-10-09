# Собственные ограниченные строки рельефа для подгрузки окна

Дата: 08.10.2026. Продолжение EP-02b/03 после
[231](231-engine-owned-unit-geometry.md), ADR-637. Контроль — frozen 231.
Критерии до изменений записаны в
`out/engine-fixes-20261008/owned-terrain-rows/acceptance.json`.

## Проблема и результат

Подгрузка окна и staging при скачках камеры напрямую читали World.column,
column_ceiling и surface_work из задач упаковки. Repair coordinator читал
World.edits и surface_changed. Это сохраняло зависимость рабочих задач
рендера от authoritative мира и мешало следующему переносу owner.

Общий `read_model::terrain` отдаёт собственные целочисленные данные:

- Header: generation/tick, размер мира и три стоимости материалов.
- Row: запрос X/Y и Z, ceilings/offsets/lengths колонок, raw u16 materials,
  разреженные позиции surface work с материалом, work и cost.
- Changes: отсортированные уникальные изменённые Z rows заданного X/Z
  прямоугольника, после наблюдаемого завершённого тика.

В них нет ссылок на World, физических очередей, Bevy, GPU формата или
названия сценария. Состояние копируется заново и на паузе; same-tick edits,
стоимости, reset и новый generation видны сразу. Старый снимок независим.
Raw materials не подменяются reserved presentation materials 9–15.
Цвета повреждений и упаковка остаются в renderer.

Строгий Request проверяется по размеру мира. Явный update_padded принимает
неотрицательное окно за его границей и представляет отсутствующий рельеф
воздухом, как прежнее чтение World. Budget ограничивает число колонок и
полный прямоугольник cells до чтения и изменения предыдущей публикации.
Сокращение копии по потолку не освобождает admission budget. Неверный запрос
или недостаточный budget возвращает Error, сохраняя прежний результат.
Все три вектора Row и bitmap/rows Changes переиспользуют вместимость.
Row копирует surface_work только внутри своего запроса; число records
ограничено cells благодаря уникальным координатам BTreeMap.

## Production consumers и сохранённая семантика

Начальное заполнение, входящие полосы при небольшом движении камеры и
восемь staging/repair rows получают данные через общий Row. Scope экспорта
заканчивается перед упаковкой. Экспорт может параллельно читать immutable
мир через существующий ComputeTaskPool; packing tasks получают только
готовые собственные строки. Порядок результатов остаётся мировым порядком
строк. Это синхронный publication boundary, а не самостоятельный sim owner.

Staging сохраняет прежние восемь строк за кадр. Входящие полосы ограничены
числом колонок восьми полных строк: максимум 8×window_width. Поэтому
полоса X шириной 8 может захватить больше Z rows, не увеличивая cell budget.
Полосы разбиваются также на тороидальной границе. Capture/pack scopes
группируют узкие строки, вместо запуска задач на каждую строку отдельно.
Не копируется весь 1280×192×1280 объём.

Staging Row inputs сохраняют вместимость между кадрами. Для входящих полос
локальные inputs переиспользуются между batches одного вызова, затем
освобождаются; reuse между отдельными сдвигами камеры ещё не введён.
Большой GPU/CPU packed staging volume, его spare и COW не заменены.

Changes ограничивает output числом Z rows окна и переиспользует bitmap.
Coordinator ставит на repair только уже прочитанные строки, как раньше.
Незаполненные читают свежий мир. Поздние dirty ranges старого видимого
объёма, включая shadows, по-прежнему входят в bounded PatchUnion.
Их replay проверяется отдельно от правильности staged CPU volume.
Observe другого generation не добавляет repairs в текущую задачу;
camera по-прежнему отменяет старую задачу перед дальнейшим step.

Смена soil/concrete/steel resistance может произойти без нового tick или
surface_changed. Staging сравнивает costs и добавляет все заполненные
строки в repair set; лимит восьми repair rows за кадр сохраняется.
Без этого уже заполненные вмятины могли публиковаться со старой стоимостью.
Вычисление оттенка work×3 теперь использует u64 перед делением, исключая
переполнение u32 без изменения результата допустимых прежних work.

Sky clearing, column tops, maximum occupied ceiling, root/segment shadows,
material colours, camera visibility/loading boundary и движение физики
во время подгрузки сохраняются. Новые данные не меняют физические budgets,
баланс, save format, AI cadence или исходы боя.

## Проверки

Семь новых engine tests проверяют raw column equivalence и независимость
данных после удаления исходного блока/повреждений; явное air padding,
сохранение строгого режима и бюджета полного padded rectangle; config costs и reuse
на том же tick; atomic rejection неправильных границ/budget; фильтрацию,
дедупликацию и reuse Changes; clipping по высоте и reset поколения.
Реальный 240-step AI бой требует shots и removed damage и сравнивает
physical hash с независимым контролем на каждом шаге. Restore на 60/200
сохраняет snapshot. Сборка публикаций не меняет save.

Две новые UI регрессии сравнивают packing с независимым per-voxel World
reference, затем меняют исходный мир до packing; проверяют repairs после
смены cost на паузе и игнорирование observe другого generation. Входные
данные workers собственные. Переполнение surface work проверено отдельно.
Прежние tests покрывают serial/2/4/8 workers, wrapping, additions/removals,
повторный repair после завершённого initial scan, surface damage, clearing
старых solids и поздние shadow uploads. Target suite включена в полную UI.

Первая полная UI suite выявила окно вне мира в прежней регрессии
staged shadows/reset: строгий export дал InvalidRequest. Исправление
добавляет явный общий air-padding contract для Row и Changes; renderer
использует его, строгие API и тест не ослаблены. Исходный отказ сохранён
в render-tests.first-error.log.

Финальная release-приёмка: **1033 headless/data pass,
0 fail, 6 ignored**; **164 UI pass, 0 fail, 4 ignored**.
Architecture: **5 pass / 4 fail**, ровно прежние **46** нормализованных
диагностик без добавлений. Старые четыре отказа остаются открытыми.
Целевые семь engine tests, шесть streaming tests и исходная padded-window
регрессия прошли; последние семь UI tests — подмножество полной suite.
`rustfmt --check` и `git diff --check` прошли. Ни один тест/эталон не ослаблен.

## Стоимость export и память

Один serial микрозамер на каждом неизменяемом early/tick3300 мире actual
stock AI боя: восемь строк шириной 1280, 200 reuse, 100 fresh capture/drop,
2000 refresh Changes. Late выбирает существующий surface work в полосе,
не создавая повреждения. Ранний и поздний cases используют разные области
по Z; их разница не изолирует цену накопления повреждений.
Clock находится в binary, вне sim library.

| Метрика | Tick0 | Tick3300 |
|---|---:|---:|
| Восемь rows reuse, mean мкс | 1197,274 | 2028,399 |
| Fresh capture восьми rows, mean мкс | 1204,558 | 2066,037 |
| Drop восьми rows, mean мкс | 0,749 | 3,312 |
| Row logical used bytes, batch | 534516 | 686528 |
| Скопированные raw cells | 143834 | 212448 |
| Sparse damage records | 0 | 616 |
| Changes refresh, mean мкс | 0,355 | 0,553 |
| Changed rows | 0 | 0 |
| Changes logical used bytes | 1368 | 1368 |

Hash до/после каждого case сохранён: early **eef062059c01815b**,
late **60bc6fb338a5ed21**. Serial capture не равен времени parallel
publication+packing и не имеет direct-copy microbenchmark прежнего
renderer; это цена нового контракта, не speedup.

Бюджет полного material rectangle восьми строк 1280×192 — 1 966 080 cells;
raw u16 bytes ≤3 932 160 плюс metadata и sparse damage. Число damage records
также ограничено этим cell budget. В entering slab число узких Z rows
может достигать window_depth, но количество колонок ≤8×window_width.
Logical bytes исключают spare capacity/allocator metadata, packed CPU/GPU
volume и retained generations; это не RSS/high-water measurement.
Общий memory budget всех consumers и lifetimes ещё не введён.

## Полный бой, native и камера

Frozen231, кандидат и late replay3300→3600 завершили по 3600 тиков.
Все **25 потиковых нетайминговых полей**, gameplay итог, shots/damage и
hash **5dd0c782d2b2f557** совпали; late replay дал тот же snapshot/hash.

| Прогон | Mean tick, мс |
|---|---:|
| Frozen231 | 13,241199 |
| Новый | 13,338844 |
| Новый с late replay | 13,323407 |

Сам terrain export не вызывается stock headless loop: эти времена
проверяют сохранение физической нагрузки, не выигрыш от publication.

Ordinary native: заранее три пары по 120 с, 1600×900, 165 FPS target,
physics/nav по 8 workers, compute 4. Все шесть runs valid/eligible/tempo,
по 3600 тиков/hash 5dd0c782d2b2f557, без reasons исключения.
Порядок и медианный gate записаны до изменения кода; все пары включены.
Builds/tests одновременно с профилями не выполнялись.

| Пара | Порядок | Control FPS | Новый FPS | Control p99, мс | Новый p99, мс | Парный gate |
|---|---|---:|---:|---:|---:|---|
| 1 | control→after | 69,9 | 68,4 | 33,562 | 34,040 | пройден |
| 2 | after→control | 67,9 | 69,4 | 33,733 | 33,587 | пройден |
| 3 | control→after | 70,0 | 68,7 | 33,436 | 33,575 | пройден |

Медианы трёх запусков каждого бинарника:

| Метрика | Control | Новый |
|---|---:|---:|
| Mean FPS | 69,9 | 68,7 |
| p95, мс | 31,163 | 31,351 |
| p99, мс | 33,562 | 33,587 |
| Медиана максимумов кадра, мс | 49,635 | 50,087 |
| Simulation realtime ratio | 0,9996 | 0,9996 |

Gate FPS≥97%, p99≤103% контроля пройден по заранее выбранным медианам:
FPS -1,717%, p99 0,074%.
Медианный FPS снизился: ускорение не подтверждено. Три коротких пары
не исключают меньшей регрессии. 165 FPS остаётся открытой целью.

Отдельная fixed camera-tour control→after пара: оба runs valid для
диагностики, но acceptance_eligible=False и meets_simulation_rate=False.
Они не выполнили 3600 тиков, поэтому финальные hash различаются и не
сравниваются как состояния одинакового tick. Эта пара не проходит
приёмку FPS и не заменяет ordinary native gate.

| Camera-tour метрика | Control | Новый |
|---|---:|---:|
| Simulation ticks | 3537 | 3566 |
| Simulation realtime ratio | 0,9819 | 0,9898 |
| Mean FPS | 72,8 | 71,5 |
| p99 кадра, мс | 38,684 | 37,848 |
| Physics-blocked frames | 0 | 0 |

| Camera operation | Runs frames control/after | Mean stream, мс control/after | p99 stream, мс control/after |
|---|---:|---:|---:|
| staging | 4714/4580 | 0,639/0,686 | 1,183/1,214 |
| publish | 27/27 | 4,936/5,278 | 12,187/12,075 |
| shift | 172/174 | 2,294/2,697 | 9,756/10,238 |

Средняя staging stream стоимость выросла, как и стоимость shift. Это
согласуется с дополнительной копией и вторым scope, но одна пара с
разным числом тиков/операций не доказывает причинность. Для следующего
шага нужны повторно используемые entering inputs и измерение capture/pack
на одинаковых tick/rectangle. Нельзя объявлять camera performance сохранённой.

Это same-thread wall durations, включая ожидание scope; не worker CPU/GPU
timestamps. Маршрут зависит от wall clock, число операций может различаться
из-за FPS и времени staging. Полный breakdown rows/shift/retire/observe,
sample counts и исходные CSV сохранены. Одна пара не доказывает causal
speedup.

`measure.ps1` впервые завершился с exit1: агрегатор потребовал eligible,
темп и 3600/hash также от заранее диагностической camera-пары. Отказ,
исходный агрегатор и обе исходные серии сохранены. Исправлен только
постпроцессинг: ordinary gate и исходные eligibility не изменены,
camera-пара явно не пригодна для FPS приёмки. Повторный `qualify.py`
проверяет структурную/headless/ordinary приёмку с exit0; profiles не
перезапускались и не отбрасывались.

## Оставшиеся границы и исправленный порядок

| Участок | Состояние после 232 |
|---|---|
| Начальное окно, entering slabs, staged/repair raw terrain | Общие owned Row/Changes; packing без ссылок на мир |
| Visible tick patches в apply_terrain | World edits, surface work и unit occupancy ещё читаются напрямую |
| Moving shadow restoration | Пишет GPU presentation, всё ещё читает physical bodies и material |
| Distant terrain и фон миникарты | Полный отдельный publication ещё не перенесён |
| Witness meshes | Owned geometry из 231 |
| Gun/poses/occupancy/history/fragments/bodies/effects | Полные отдельные payloads ещё открыты |

Следующий срез — собственные visible terrain patches с реальными
материалами/work/cost и последовательностью world/surface/unit-removal
изменений. Occupancy и bounds должны перейти через общий контракт, без
клонирования тел армии. Цена camera streaming требует отдельного среза:
переиспользование entering inputs между сдвигами и измерение capture/pack
на одинаковых tick/rectangle до выводов о производительности. Затем
bounded distant/minimap reads и остальные
geometry/effects. Changes не является журналом: каждый завершённый tick
нужно наблюдать; потерянные публикации восстанавливаются полной/region
resync. Долговременные delta cursor/lifecycle и общий coherent export
не следует объявлять завершёнными по одному row consumer.

Scan physical edit lists и X-range retained surface history остаётся
линейным в соответствующей истории; spatial/page deltas потребуют отдельной
работы и измерений. EP-02/03 и WBPERF-002/020 остаются частичными.
Owner вне кадра, async lifecycle/cancellation, release/wait dependencies,
прогресс миллионных физических очередей и 165 FPS ещё открыты.

## Воспроизводимость

Все 412 входов frozen 231 проверены до изменений. Backups изменённых
исходников, scripts, fixed acceptance, тестовые и измерительные logs,
raw и qualification лежат в `out/engine-fixes-20261008/owned-terrain-rows/`.
Build locked/offline/release инкрементальный; принудительный source refresh
и независимый SDK rebuild не выполняются, предел указан в passports.
Данные, зависимости и golden files прежние.


Заморожено 415 входов; изменённые относительно 412 исходники перечислены
в qualification.json. SHA-256 binaries:

| Бинарник | SHA-256 |
|---|---|
| 231 bench | `A40292A9F8EB148B0EBE0B8BF019CF5E83AA1569730388CB82AA4CEDDAC6D7A4` |
| 232 bench | `F170599A1D6E1AE2A8651F68611BF7655FF756E3788B523D24FE562C707C1D1D` |
| 231 native | `7DDBA8E7B92B90EDE035A923BDB5633DA8722C028701112203C10EF443491C11` |
| 232 native | `871FC567D8FFC635A89D6ED11347D004E5BBCD9161A145FB13E1AA3C17589B10` |
| 232 terrain benchmark | `1D890272AD1637EA8C9888154D7CE0C973A7AE5BE9517B3515871DF5BB80485D` |
