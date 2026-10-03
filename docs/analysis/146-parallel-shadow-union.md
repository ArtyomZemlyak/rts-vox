# Параллельное слияние масок теней — ADR-552

03.10.2026. Продолжение ADR-551 / WBPERF-007. Actor jobs уже готовили
sorted 64-cell masks параллельно, но caller один сливал все списки через
min-heap и перечислял каждый бит в плоский массив клеток. Этот хвост
оставался на критическом пути после join подготовки.

## Изменение

`moving/packed.rs` делит word domain на максимум четыре непересекающихся
диапазона. Границы находятся binary search по сумме prefix lengths
actor lists; это деление по числу входных masks, а не по расстоянию на
карте. Одна плотная группа в углу не получает весь диапазон координат
одного задания. Одинаковый word никогда не делится между заданиями:
каждый worker выполняет прежние OR/min-heap и ascending bit enumeration
для своих slices. Caller копирует результаты в порядке диапазонов.
Final flat union, GPU nibble writes и dirty spans остаются прежними.

Используется существующий ComputeTaskPool; новых потоков/пулов нет.
Parallel merge включён от четырёх threads и 32768 input masks, число
leaves ограничено четырьмя. Меньшие нагрузки сохраняют serial merge;
serial/1/2 actor preparation сохраняет прежний radix path ADR-551.
Порог консервативный, его оптимальность для каждого состава не доказана.
Задания читают immutable prepared actor lists и владеют разными
`Vec<usize>`; их capacity переиспользуется и сохраняется при reset.
Никакой полной копии мира/симуляции или задержки видимых изменений нет.

## Изолированный CPU test

Before binary заморожен до edits. Идентичный ignored benchmark:
60 actors всех семи kinds, 120 меняющихся poses, окно 640×192×640.
Запуски последовательны, без сборок, тестов или тяжёлого анализа рядом.

| Compute workers | До, mean мс | Prototype, mean мс | Final, mean мс |
|---|---:|---:|---:|
| 0 | 28,951 | 29,322 | 28,854 |
| 1 | 21,410 | 21,766 | 21,359 |
| 2 | 19,045 | 19,033 | 19,808 |
| 4 | 15,259 | 12,266 | 11,892 |
| 8 | 12,760 | 9,762 | 9,672 |

В final сокращение всей cached occupancy preparation/union: 22,1% на
четырёх threads, 24,2% на восьми. Prototype давал 19,6/23,5%.
Во всех вариантах final union содержит 2164873 клетки. Packed actors
сохраняют 2621184 cells / 206904 masks; radix path — 2828454 raw cells.
Это CPU microbenchmark, не FPS. На неизменённом two-worker path final
время выше на 4%; разброс малых пулов не выдаётся за выигрыш.

## Ordinary full battle

Четыре frozen native прогона по 120 секунд, порядок before → after →
after-repeat → before-repeat. 30×30 units, AI с обеих сторон, physics/nav
8/8, Compute 4, 1600×900 AutoNoVsync. Core/render task collectors выключены;
обычный process sampler включён. Во время измерений не запускались
сборки, тесты, редактирование или тяжёлый анализ.

| Метрика | До / 1 | После / 1 | До / 2 | После / 2 |
|---|---:|---:|---:|---:|
| FPS | 85,753 | 86,453 | 86,141 | 86,213 |
| Mean frame, мс | 11,661 | 11,567 | 11,609 | 11,599 |
| p99 frame, мс | 30,738 | 30,511 | 30,746 | 30,758 |
| Max frame, мс | 63,021 | 65,214 | 65,960 | 64,630 |
| Occupancy / completed step, мс | 2,038 | 1,995 | 2,052 | 1,997 |
| Occupancy / frame p99, мс | 3,278 | 3,207 | 3,283 | 3,174 |
| Pure core / step, мс | 14,036 | 13,963 | 14,021 | 14,030 |

Occupancy / step дешевле на 2,1% и 2,7%, p99 этой фазы также ниже.
Overall FPS +0,82% / +0,08%; повтор почти равен baseline. Устойчивый
overall FPS gain не установлен. Whole-frame p99/max не улучшаются в обеих
парах, устранение stalls не заявляется. Core не изменён, его скорость
не объявляется улучшенной. Более сильный CPU gain относится к плотному
составу microbenchmark, не переносится на обычный native бой.

Все четыре acceptance eligible / valid, выполнили 3600 steps, ratio
0,99970…0,99976, hash `bcc1f58c226d1721`, alive [23,25]. Полные строки
shots/damage, stream packets, melted cells и combustion evidence совпали.
Набор юнитов, эффекты, камера, визуальные настройки и частота боя прежние.

## Проверки и оставшиеся ограничения

Full release `render --bin blast`: 114 passed, 2 ignored. Ignored
occupancy benchmark выполнен отдельно на frozen final test binary.
Release build/Clippy/own fmt проходят; пять прежних warnings сохраняются,
новых нет. Существующие occupancy differential tests покрывают
serial/1/2/4/8, movement/damage/legs/appearance, window shifts/re-entry,
resize/reset/roster/mode switches. Новые tests проверяют actual range jobs
на 1/4/8: disjoint formations, overlap, skew, empty actors, empty roster,
append semantics, full usize cell boundaries, repeated buffer reuse,
production gate и nested pool на 1/4. Результат равен serial union.
Полный CI / прежний architecture debt этим не закрыты.

Input mask count не учитывает разную плотность bits и overlap, поэтому
деление не доказывает равную длительность jobs или загрузку CPU.
Worker OS IDs/cycles и отдельные prepare/sort/pack/merge/copy spans
в native не сняты. Whole-process CPU ~2,91…2,97 core equivalents не
доказывает равномерность и не позволяет назначить cost этим jobs.
Существующие агрегаты occupancy показывают net фазу, включая scheduling,
подготовку, merge и commit; доля включённых parallel merges отдельно
не записана. Эта наблюдаемость остаётся задачей WBPERF-001/007/027.

Final copying/GPU commit остаются serial. Дополнительные part buffers
удерживают union-sized capacity: для 2164873 cells полезный payload около
16,5 MiB, allocator capacity и whole-process residency не измерены.
Синхронный тик ~14 мс по-прежнему блокирует Update: WBPERF-002, rare
stalls и 165 FPS открыты. Частичная ускоренная фаза не закрывает весь бой.

## Evidence и provenance

Исходный HEAD `076fcd9f4ac7a9f7459a584a887ff4ee3e1d9d84`. Оба билда содержат
одинаковую незавершённую ADR-523 selection/laser/feet правку. Это не clean
Git snapshots: GitHead/SourceAtLaunch не доказывают built source.

- `out/fix552/before.exe`: SHA256
  `99630cbb0be958cc8362924db151d42e71554f3bee3fe4ef6e27e338d4fb0b56`.
- `out/fix552/after.exe` / `target/release/blast.exe`: SHA256
  `5236bd739a818e495386b600339a0008564377b185119b1ae0c3310a959f3c91`.

CPU logs: `bench-before.log`, `bench-parallel.log`, `bench-final.log`.
Проверки: `tests-moving.log`, `tests-render.log`, `clippy.log`, `build.log`.
Сохранены before/prototype/final test binaries. Native CSV/log/manifest,
process samples и analysis JSON — в
`out/fix552/{before,after}-native{,-repeat}-120/`. `summarize.py` требует
comparison gates, 3600 steps и совпадение полного gameplay evidence;
агрегаты — `native-summary.json`, `native-summary.print.txt`.
Out evidence не архивируется в Git.
