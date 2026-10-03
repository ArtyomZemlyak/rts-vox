# Параллельная упаковка и точное слияние теней юнитов — ADR-551

03.10.2026. Продолжение WBPERF-007; после ADR-550.

## Ограничение

`moving::occupancy` уже готовил геометрию отдельных юнитов в ComputeTaskPool.
Затем вызывающий поток собирал все клеточные адреса в один массив и сортировал
его двумя radix passes с дедупликацией. На плотном стенде это 2,83 млн адресов,
включая повторы. В свежем нативном полном бою occupancy стоил 2,88…2,89 мс
на шаг. `terrain_ms` также включает occupancy; отдельно apply_terrain в
прошлом контроле занимал лишь около 0,066 мс/кадр. Переносить его в пул
до измерения более дорогой части не стали.

## Изменение

В существующем actor job клеточные адреса сортируются и упаковываются в
`(word_index, u64_mask)`: каждый бит обозначает ровно одну прежнюю клетку.
Маски переиспользуются до изменения геометрии/позы/clipping. После join
вызывающий поток сливает отсортированные списки слов через min-heap и OR,
затем перечисляет установленные биты в прежнем порядке адресов.
В плотном стенде это 206904 слов вместо 2828454 исходных клеток;
конечный union содержит прежние 2164873 клетки.

Результат не упрощается: корень, повёрнутая башня, все уцелевшие сегменты,
дальняя граница окна, перекрытие юнитов и toroidal addresses прежние.
Последующее сравнение old/new cells, material restoration, dirty spans,
GPU payload и revision acknowledgement не меняются. Физика, пулы, частота
симуляции, игровой баланс и snapshot остаются прежними.

Packed path применяется при ≥4 actors и ≥4 threads существующего ComputeTaskPool.
Меньшие составы/пулы используют прежний concatenation/radix путь.
Переключение режима восстанавливает clipped cells из неизменного world-cell
cache; уход за окно очищает также packed masks. Cache не сериализуется.

## Отвергнутый вариант и CPU-стенд

Сначала packing применялся всегда. Это ухудшило serial/малые пулы:
без пула 29,495→44,949 мс, один поток 21,618→26,310 мс, два
19,106→19,920 мс. Такой вариант не оставлен штатным. Вариант с выбором
по числу потоков проверен отдельно; baseline снят до изменений.

60 юнитов всех семи типов, 120 меняющихся поз на каждое число потоков,
640×192×640 window, isolated ignored headless benchmark. Это плотный
occupancy test, а не нативный бой/FPS.

| Threads | До, mean мс | Always-packed prototype | Принятый adaptive |
|---|---:|---:|---:|
| Без пула | 29,495 | 44,949 | 29,136 |
| 1 | 21,618 | 26,310 | 21,509 |
| 2 | 19,106 | 19,920 | 18,971 |
| 4 | 17,906 | 14,916 | 15,261 |
| 8 | 16,356 | 12,501 | 12,676 |

На 4/8 mean сократился на 14,8/22,5%; малые пулы сохраняют прежний алгоритм.
Малые различия 0…2 не приписываются новой оптимизации. Бенч теперь сообщает
cached cell count и packed word count, чтобы видеть размер подготовки/слияния.
Оптимальный режим для любого неравномерного состава этим не доказан.

## Нативный полный бой

Четыре ordinary 120-секундных запуска: before → after → after-repeat →
before-repeat. Штатный бой 30×30, ИИ обеих сторон, physics/navigation 8/8,
прежний compute default 4, 1600×900 AutoNoVsync. Binary/input hashes,
process/environment gates и offline analyzer прошли. Core/render diagnostic
collectors выключены. Во время измерений не запускались сборки, тесты,
редактирование или тяжёлый анализ.

| Метрика | До / 1 | После / 1 | До / 2 | После / 2 |
|---|---:|---:|---:|---:|
| FPS | 80,262 | 85,434 | 81,264 | 86,718 |
| Mean frame, мс | 12,459 | 11,705 | 12,306 | 11,532 |
| p99 frame, мс | 31,852 | 30,770 | 31,373 | 30,642 |
| Max frame, мс | 63,062 | 63,122 | 63,514 | 64,215 |
| Occupancy / completed step, мс | 2,881 | 2,051 | 2,891 | 2,047 |
| Occupancy / frame mean, мс | 1,077 | 0,720 | 1,067 | 0,708 |
| Occupancy / frame p99, мс | 3,834 | 3,282 | 3,870 | 3,264 |
| Pure core / step, мс | 14,041 | 14,006 | 14,086 | 13,974 |

FPS вырос на 6,44% и 6,71%. Occupancy / step сократился на 28,8% и 29,2%:
это отделяет выигрыш от уменьшения доли sim frames при большем FPS.
Mean/p99 кадра улучшились в обеих парах, но максимальный кадр стал немного
дороже в обеих. Снижение всех задержек не заявляется. Неизменённые фазы
также имеют межзапусковой разброс; весь FPS-выигрыш не равен измеренному
сокращению одной CPU-фазы.

Все запуски выполнили 3600 шагов, ratio 0,9996…0,9997, сохранили
`bcc1f58c226d1721`, alive [23,25], shots/damage, stream packets,
melted cells и combustion counters. Их evidence сравнивается целиком.
Камера/частота боя/визуальные эффекты и набор юнитов не упрощались.

## Проверки и границы

Полный release `render --bin blast`: 112 passed, 2 ignored. Игнорируемый
occupancy benchmark выполнен отдельно. Матрица 1/2/4/8 сравнивает final union
с прежним sorted reference и свежей геометрией: movement/rotation, повреждения,
потерянная нога, appearance-only revision, shifts/re-entry, resize, reset,
добавление/удаление actors. Кэши сравниваются после canonical packing, потому
что исходный порядок клеток законно отличается в двух алгоритмах.
Смена пула serial→4→1→8→serial→4 сохраняет точную маску без stale shadows.
Nested scope на 1/4 завершает работу; pure mask tests покрывают duplicates,
overlap, empty lists, 64-bit boundaries и полный usize address range.

Release Clippy/build и собственный fmt проходят. Пять прежних предупреждений
voxel/nav/sim-core/render не исправлялись; в новых файлах предупреждений нет.
Полный CI/старый architecture debt этой проверкой не закрываются.

Первый HEAD `0e6b10d2553f2a13d4e11c66b2f33ab6ca5ade3c`. В обеих сборках
сохранена одинаковая незавершённая ADR-523 selection/laser/feet правка.
Это не clean Git snapshots; metadata GitHead/SourceAtLaunch не доказывает
built source. Frozen native binaries:

- `out/fix551/before.exe`: SHA256
  `3c7f169194b952fef72b8ee02569fddd81d7e260deaaf611b3248e7d22a5931c`.
- `out/fix551/after.exe` / `target/release/blast.exe`: SHA256
  `99630cbb0be958cc8362924db151d42e71554f3bee3fe4ef6e27e338d4fb0b56`.

Raw native evidence находится в `out/fix551/{before,after}-native{,-repeat}-120/`;
агрегат — `native-summary.json`, проверка — `summarize.py`. CPU:
`bench-before.log`, `bench-after.log` (always-packed), `bench-adaptive.log`.
Проверки/сборка — `tests-moving*.log`, `tests-render.log`, `clippy.log`,
`build.log`. Сохранены before/after test binaries. Out-файлы не архив Git.

WBPERF-007/002/027, цель 165 FPS и rare stalls остаются открытыми.
Тик симуляции всё ещё блокирует Update. Actor sorting/packing исполняются
в прежних worker jobs; merge и GPU-word commit последовательны.
Worker-level OS IDs/cycles для occupancy не сняты, равномерная загрузка
CPU не доказана. Отдельные prepare/sort/pack/merge spans и размеры
конкретных actor jobs — следующий шаг наблюдаемости этой фазы.
Packed words удерживают capacity; whole-process memory/residency не закрыты.
