# ADR-554 — локальные доказательства под корпусом и проверяемые сборки

04.10.2026, исходный HEAD `64785f2`, WBPERF-003/004/001/027.
Артефакты: `out/fix554`, frozen binaries, build manifests, logs/CSV,
`summarize-core.py`, `summarize-native.py`, `core-summary.json`,
`native-summary.json`. Измерения последовательные, без конкурирующих
сборок, тестов, правок или тяжёлого анализа во время каждого прогона.

## Принятое изменение геометрии

Один resting debris voxel в bounding box широкого корпуса прежде отключал
все tile proofs его belly. Теперь каждая подготовленная плитка отдельно
проверяет отсутствие rubble. Подтверждённая bare upper bound ниже минимальной
плоскости belly позволяет пропустить её; плитки с rubble, yielding matter,
неподготовленными/изменившимися данными остаются точными запросами колонок.

Контакты, наклон, высота, blocked/crush counts, fixed-point rounding,
порядок обхода оставшихся колонок и logical search allowance прежние.
Границы мира не проходят этот shortcut. Общий early return по bare upper
по-прежнему требует отсутствия rubble во всей области. Кеш не переживает
изменение мира: invalidated tile возвращает None и требует exact fallback.

Нет новых потоков/пулов, лимитов работы, изменений баланса, частоты симуляции,
изображения или сохранений. Изменение снижает работу уже существующих
навигационных воркеров. Это локальное ускорение, не завершение полной
оптимизации боя и не доказательство устойчивого native FPS выигрыша.

## Ошибка, мешавшая сравнению вариантов

`Copy-Item` переносит LastWriteTime исходника. Восстановленный файл может
оказаться старше ранее скомпилированного объекта, и Cargo сочтёт его fresh.
Это подтвердилось в первом cold-pose микротесте: supposedly old/new binary
имели одинаковый SHA-256 `96347b3fcf98ed9740dcfc1a6bd0235699e590cbfc1a80ba32c95299c568e566`,
а второй build завершился за 0,09 с. Такие данные не сравнивают два алгоритма.

Добавлен `scripts/build-range-variant.ps1`: для **всех перечисленных**
скопированных/восстановленных файлов ставит свежее время, снимает SHA до
сборки, проверяет неизменность содержимого после неё, сохраняет executable
и `.build.json`. Существующий artifact не перезаписывается. Caller обязан
перечислить и отменённые эксперименты; это не автоматическое обнаружение
всех inputs и не полная аттестация dependency closure/clean Git checkout.

`profile-range.ps1` переносит optional build evidence в run manifest и
отклоняет несовпадающий hash, неизвестную schema или сломанный JSON.
Legacy frozen binaries без `.build.json` сохраняют явно неизвестное build
provenance; SourceAtLaunch/HEAD не объявляются доказательством built source.

Пример из PowerShell с обходом локального ограничения запуска scripts:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -Command "& './scripts/build-range-variant.ps1' -Package render -Binary blast -OutputBinary out/my-variant.exe -RefreshSources @('crates/nav/src/vehicle.rs','crates/sim-core/src/blast.rs','crates/sim-core/src/range_motion.rs')"
```

Список расширяется на каждый реально восстановленный input, включая .ron
и shaders. Содержимое остальных файлов и режим/flags сборки также должны
совпадать при A/B. Перед bench необходимо закончить все сборки/тесты.

## Контролируемый cold-pose benchmark

`cargo run --release -p nav --bin bench_vehicle_pose` — отдельный воспроизводимый
CPU workload. Наклонная подготовленная поверхность 384×96×384, широкий
корпус, четыре headings и дробные positions. Четыре случая вращаются по
порядку; 20 warm rounds + 200 measured, 64 pose/round. Время включает
создание/освобождение cold Probe, geometry и вычисление result hash;
генерация мира, подготовка tiles и log I/O вне таймера. Это **не** тёплый
длительный Hunt, не физика всего боя и не native FPS.

В двух verified builds алгоритмы используют одинаковый fixture и FNV hash
всех result fields. Старый rotate/add hash экспериментального bench заменён:
он давал одинаковую итоговую величину даже для различавшегося high-heap
случая; этот ранний digest не используется как доказательство равенства.

| 64 cold poses, среднее мс | До | После | Columns до → после |
|---|---:|---:|---:|
| Без обломков | 1,053643 | 1,013710 | 78508 → 78508 |
| Один низкий heap | 17,609133 | 1,471114 | 1903865 → 82604 |
| Один высокий heap | 17,539897 | 1,479897 | 1903865 → 82604 |
| Три sparse heaps | 19,033780 | 1,736751 | 1903865 → 90795 |

Heap случаи быстрее примерно в 11–12 раз; bare разница ~4% не является
устойчиво доказанным ускорением этого неизменённого пути. Hash каждого
соответствующего случая совпал; high-heap digest отличается от bare/low,
потому что реальная блокировка сохраняется. Checks сравнивают весь Pose с
полным пробированием без prepared-profile shortcut, включая высокие heaps,
положение/разворот, изменения материалов и yielding matter.

Verified binaries:
- `pose-before-verified.exe`: `40bd874b3b6fad8d8134f8abca83940b2cc23182e665c8532a734f9b09332aa1`;
- `pose-tiles-verified.exe`: `a24934c5dcf86d9262b228e7c0604e2d395ffa0a31913b3dc50b4eb15882b1ef`.

## CPU полный бой

30×30, AI обеих сторон, physics/nav=8, по 3600 тиков; только bench
phase clocks, без worker cycle observer. Порядок before→after→after→before.
Перед этими verified сборками refreshed nav и оба восстановленных sim-core
исходника. Every-tick counters searching/moving/blocked/fragments/flying/
chunks/felling, полный final state/shot/damage/alive/bases и per-unit
диагностические состояния совпали. Hash `bcc1f58c226d1721`.

| Среднее мс | До / 1 | После / 1 | До / 2 | После / 2 |
|---|---:|---:|---:|---:|
| Полный тик | 14,450571 | 13,491443 | 14,225971 | 13,373067 |
| Motion | 5,723103 | 5,192111 | 5,374041 | 5,162230 |
| Routes | 3,191913 | 2,877944 | 3,002488 | 2,865143 |
| Feet | 0,897443 | 0,743408 | 0,767761 | 0,740788 |
| p99 tick | 29,812 | 22,884 | 24,151 | 22,068 |
| Max tick | 172,863 | 50,669 | 53,032 | 50,053 |

Motion −9,3% / −3,9%; routes −9,8% / −4,6%. Общий tick −6,6% / −6,0%
в этих двух парах, но часть разницы относится к неизменённым фазам;
первый before имеет дополнительный 172,863 мс peak. Не приписывать всю
разницу shortcut и не удалять этот пик из статистики.

Core binaries:
- before `f2c9296a1cbd96df862752bfd6a4fe9683596e11488ff081edbfdd22ecb83b1c`;
- after `efc6ca34fa36bbe1cc41565f292534913c448332d33478a9f8e74f8dd56054b6`.

## Обычное окно — общий FPS выигрыш не подтверждён

Четыре последовательных runs по 120 с, before→after→after→before,
1600×900, AutoNoVsync, compute=4, physics/nav=8, штатные эффекты и sampler
процесса. Detailed collectors выключены. Все native/offline comparison
gates проходят; по 3600 steps, tempo 0,99960…0,99974×. Full gameplay
log evidence одинаковы, тот же hash. Строгая приёмка условий не означает
приёмку цели 165 FPS или причинной атрибуции ускорения.

| Метрика | До / 1 | После / 1 | До / 2 | После / 2 |
|---|---:|---:|---:|---:|
| FPS | 73,8074 | 77,7547 | 75,0621 | 72,3513 |
| Mean frame, мс | 13,548770 | 12,860958 | 13,322296 | 13,821444 |
| p99 frame, мс | 33,0303 | 32,3022 | 33,8660 | 34,7127 |
| Max frame, мс | 64,0862 | 63,9843 | 69,5739 | 68,1931 |
| Core/step, мс | 15,177934 | 14,970812 | 15,387905 | 15,490988 |

Первая пара FPS +5,35%, вторая **−3,61%**. Core −1,36% / +0,67%.
Whole-frame p99 улучшился в первой, ухудшился во второй. Устойчивого
ускорения всего боя нет, наблюдаемая просадка второй пары — открытый риск.
Локальный shortcut принят по cold-pose и повторяемому CPU motion результату;
не объявлять его исправлением native stalls или готовностью полного боя.
Нужна дальнейшая native attribution критического пути и gate hits; здесь
нет отдельного tile-proof scope/OS cycles/residency. Не объяснять различия
внешней нагрузкой без OS доказательства. Процесс 2,80–2,87 core equivalents
не доказывает balanced CPU или недостаточное число потоков.

Native control — frozen ADR-553 `before.exe`, SHA
`7bc2a6824cb5e733292b03f26c6d66ee73e03839cef50d675bf9a5a43743156b`,
без build manifest новой процедуры. Новый `tiles-verified.exe` и
`target/release/blast.exe`:
`39018510322501891f2e06a68c05cbf6fc5bd736560b22349a25cdef1c4b8821`.
Оба содержат один и тот же прежний unrelated ADR-523 WIP; это не clean
Git checkout. Source hashes и frozen artifacts проверяют ограниченный
контракт сборки, не превращают HEAD в доказательство всех зависимостей.

## Прототипы и исключённые ранние выводы

1. Sparse Z bounds над rubble columns: дерево отсекало участки bbox.
   Первичная CPU пара 13,415→13,590 мс не дала выигрыша. Удалено; отдельная
   индексная перестройка не входит в итоговый алгоритм.
2. Вложенные pure geometry jobs у local spider hunt, ordered blocker journal
   и live revalidation сохранялись. Полная промежуточная suite 561/4 ignored
   прошла. Первый CPU run имел tick2686 534,741 мс, одновременно motion,
   debris, rubble и laser стали дороже; причина не атрибутирована. Native
   77,4106→77,5858 FPS, p99 32,1871→34,0675. Ранний binary мог содержать
   прежний nav prototype из-за restored timestamps: **не считать это
   изолированной проверкой local parallelism**. Код удалён; после нового
   build protocol нужен отдельный эксперимент, если вернёмся к кандидату.
3. `with_max_len(1)` у route/ground/feet, без перестановки юнитов из
   отклонённого ADR-513. Первичный CPU motion 5,168→5,139 мс, прирост мал;
   также не использовать эту пару как чистую аттестацию restored nav.
   Удалено, production scheduling прежнее.
4. Первые cold-pose before/after были одним executable. Все pre-verified
   сравнения из этой серии сохраняются в out, но не входят в приёмку.
   Первичный `tiles-bench` 13,308 мс тоже не используется как verified pair.

Ошибку Copy-Item нельзя задним числом приписывать всем прошлым ADR:
для каждой старой серии требуются её source/build logs и hashes.

## Проверки и следующий приоритет

- nav release lib: 101 passed;
- окончательный sim-core release lib: 560 passed, 4 прежних ignored;
- PowerShell profile tools: 54 cases и parser checks;
- Python range analyzer: 95 passed;
- Clippy release all-targets проходит с существующими предупреждениями,
  новый benchmark предупреждений не добавил; fmt own Rust paths проходит;
- source refresh реально заставил собрать два разных micro executables;
  профиль переносит matching build evidence, отклоняет mismatch/schema/JSON.

Architecture CI debt из предыдущих обзоров не закрывается этим изменением.
WBPERF-002 (владелец симуляции вне frame), 027 (стэки/ожидания длинных
кадров), 003/004 (native route/geometry attribution), 001 (полнота
наблюдаемости) и 165 FPS остаются открыты. Следующий главный шаг —
измерить синхронный critical path и native cost/gate hits, затем переносить
симуляцию по compact read model и bounded commands/event journal;
не делать full BlastRange copy на кадр/тик.
