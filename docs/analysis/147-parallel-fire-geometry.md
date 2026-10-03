# Подготовка слоёв огня и дыма — ADR-553

03.10.2026. WBPERF-020/025, продолжение оптимизаций полного боя.
`fire::draw` собирал два независимых translucent MeshBuf последовательно,
затем `mesh(&buffer)` копировал позиции, нормали, цвета и индексы.
Широкая presentation group также включает другие системы; её стоимость
нельзя целиком приписывать огню или HUD.

## Принятый вариант

`fire/geometry.rs` получает immutable references одного render frame:
симуляцию, Effects, уже отсортированные частицы и interpolated Witness
transforms. Порядок camera sort, фильтрация возраста, bloom/materials,
число частиц, cube/beam geometry, shader settings и время эффектов прежние.
Внутри каждого слоя сохранён исходный порядок: weapon traces / rounds,
его particles, затем thermal glow для hot; camera-ordered particles для
smoke. Глобальная сортировка и сбор Witness transforms остаются на caller.

От двух Compute threads, 32 primitives в каждом слое и 256 суммарно
caller строит больший оценённый слой, пока одно scope job готовит меньший
в существующем ComputeTaskPool. Count estimate учитывает particles,
tracers, lasers, rounds и heat spots; это не измерение реальной длительности.
Оба слоя в этом пути передают Vecs в Mesh по владению. Никакого нового
пула, snapshot мира или задержки публикации нет. Assets/Visibility
меняются только на caller после join. Меньшие/serial нагрузки сохраняют
компактную cloned публикацию; порог не объявляется оптимальным.

Normal scope stealing сохранён для безопасного nested use. Caller сначала
исполняет свой builder, вместо немедленного входа в ожидание двух jobs.
Снижение длительности ожиданий не измерено отдельно, поэтому механизм
выигрыша и причину всех прежних tails это изменение не доказывает.

Пустой слой возвращает None: последний nonempty Mesh остаётся hidden.
Начальная tiny hidden cube заглушка Bevy allocator прежняя. Owned Mesh
сохраняет позиции, нормали, цвета, индексы и optional UV/tangent, а также
MAIN_WORLD | RENDER_WORLD usage. Дополнительные буферы состояния мира не
создаются; growable Vec capacity в owned meshes может превышать длину.

## Прототипы и исправление измерения

Первый вариант отправлял оба слоя в jobs и применял owned conversion
также serial. Один ordinary native run: 88,376 FPS против 86,510 baseline,
presentation mean 0,717 против 0,775 мс, но p99 1,707 против 1,300 мс.
Это не повторённое доказательство overall gain. Он не принят.

Второй вариант вернул compact serial publication, оставив два jobs:
86,976 FPS, presentation mean 0,727 / p99 1,590 / max 4,259 мс.
Whole frame max 78,078 мс. Этот вариант также не принят.

Первые isolated timings сравнивали блоки в фиксированном порядке без
прогрева и opaque inputs; после пула запускался direct cloned reference.
Они показывали ownership-only regression, но не отделяли порядок,
allocator warm-up и возможную compiler specialization. Причины разброса
не доказаны. Такие цифры не используются для итоговых сравнительных
выводов. Bench исправлен: все pools создаются вне timer, каждый путь
прогревается, семь cases чередуются, Input проходит black_box, reference
builders имеют inline(never). После этого serial/reference существенно
ближе; owned reference оказался быстрее cloned. Ранний вывод о пользе
копирования как общей оптимизации отозван. Compact fallback сохраняет
прежний small/serial режим, его превосходство не установлено.

Pinned Bevy 0.19.1 `task_pool.rs::scope` включает global executor ticking;
upstream documentation прямо допускает исполнение unrelated tasks во время
join. Это гипотеза источника tails, не установленная причина наблюдавшихся
пауз. `scope_with_executor(false)` не вводился: сохранена помощь executor
для nested callers. Принятый вариант даёт caller полезную работу до join.

## Контролируемый CPU test

CAP=1800 mixed particles всех flame/dust/smoke branches, tracers, laser,
thermal spots root/segments семи kinds. Camera-ordered input и poses
одинаковые. 20 warm rounds, 200 measured samples на каждый case, порядок
вращается, без конкурирующих сборок, тестов или native runs.

| Путь | Mean, мс | p99, мс |
|---|---:|---:|
| Production, no pool | 0,521 | 1,269 |
| Production, 1 thread | 0,540 | 1,341 |
| Production, 2 threads | 0,295 | 0,792 |
| Production, 4 threads | 0,282 | 0,780 |
| Production, 8 threads | 0,253 | 0,739 |
| Extracted cloned reference | 0,530 | 1,544 |
| Extracted owned serial reference | 0,419 | 1,280 |

4-thread preparation дешевле production serial на 45,8%; 8-thread — на
51,4%. Reference использует те же extracted builders и старый cloned
converter, а не frozen полную старую draw system. Sorting, pose capture,
Asset replacement, GPU extraction/upload и Mesh drop после timer не
включены. Это не FPS и не доказательство native small/serial performance.

## Ordinary full battle: принятый caller variant

Четыре frozen native runs по 120 секунд, control before → caller →
caller-repeat → before-repeat; между первым control и caller выполнены
описанные prototype runs и проверки. Вторая пара исполнялась подряд
в обратном порядке. 30×30 units, AI обеих сторон, physics/nav 8/8,
Compute 4, 1600×900 AutoNoVsync. Detailed core/render collectors выключены,
ordinary process sampler включён. Во время измерений не запускались
сборки, тесты, редактирование или тяжёлый анализ.

| Метрика | До / 1 | После / 1 | До / 2 | После / 2 |
|---|---:|---:|---:|---:|
| FPS | 86,510 | 86,824 | 85,851 | 85,985 |
| Mean frame, мс | 11,559 | 11,518 | 11,648 | 11,630 |
| p99 frame, мс | 30,526 | 30,866 | 30,674 | 30,797 |
| Max frame, мс | 67,738 | 61,334 | 68,462 | 62,302 |
| Presentation mean, мс | 0,775 | 0,685 | 0,774 | 0,681 |
| Presentation p99, мс | 1,300 | 1,193 | 1,309 | 1,166 |
| Presentation max, мс | 2,307 | 2,014 | 2,261 | 2,066 |
| Presentation mean после 30 с, мс | 0,885 | 0,788 | 0,885 | 0,777 |
| Pure core / step, мс | 14,130 | 14,211 | 14,144 | 14,185 |

Presentation group mean −11,6/12,0%, p99 −8,2/10,9%, max также ниже
в обеих парах. Внутренние spans fire/worker/join не сняты: это net группы,
а не назначенная целиком fire duration. Overall FPS +0,36/0,16% близок
к разбросу, убедительный FPS gain не заявляется. Whole-frame p99 немного
хуже в обеих парах; max ниже, но устранение rare stalls не доказано.
Pure core / step немного дороже, ускорение симуляции не заявляется.

Все четыре comparison valid / acceptance eligible, 3600 completed steps,
ratio 0,999649…0,999682, hash `bcc1f58c226d1721`, alive [23,25]. Полные
строки shots/damage, stream packets, melted cells и combustion evidence
совпадают. У обоих прототипов те же gameplay evidence в их первых парах.

## Проверки и границы

Final full release `render --bin blast`: 117 passed, 3 ignored; geometry
bench выполнен отдельно. Geometry tests сравнивают все Mesh attributes,
indices и usage с cloned reference на no-pool/1/2/4/8, battle/range,
Laser/MachineGun/Kinetic и counts 0/1/31/32/255/256/CAP. Fixture содержит
все семь kinds, root/segment thermal spots, interpolated transforms,
smoke/dust/fire/stream branches и фиксированный camera order с ties.
Nested scopes 1/2/4 завершаются, пустые слои отсутствуют. Pointer check
подтверждает отсутствие копии owned positions; optional UV/tangent сохранены.
Renderer math literals перенесены без изменения. Это не screenshot oracle
всей прежней картинки; новые tests прежде всего доказывают conversion
и serial/parallel parity. Existing effects/HUD/motion tests сохранены.

Release build/Clippy и own fmt проходят; пять прежних warnings остаются,
новых нет. Полный CI/прежний architecture debt не закрыты.
OS worker IDs/cycles, queue/join waits, native gate hit counts и residency
не измерены. Process CPU 2,93…3,00 core equivalents не доказывает баланс
загрузки. Witness pose capture, sorting, Mesh allocation, ECS publication
и другие presentation systems требуют отдельных spans. WBPERF-020/025
частичны, WBPERF-001/027 открыты. Синхронный core tick остаётся в Update;
WBPERF-002 и 165 FPS не выполнены. GPU work и workload не уменьшались.

## Evidence / provenance

Исходный HEAD `fc4263209cf79444a5b479ee4bbd5337ec1626fe`. Все билды содержат
одинаковую незавершённую ADR-523 selection/laser/feet правку. Это не clean
Git snapshots; GitHead/SourceAtLaunch не доказывают built source.

- `out/fix553/before.exe`: SHA256
  `5236bd739a818e495386b600339a0008564377b185119b1ae0c3310a959f3c91`.
- `out/fix553/caller.exe` / `target/release/blast.exe`: SHA256
  `7bc2a6824cb5e733292b03f26c6d66ee73e03839cef50d675bf9a5a43743156b`.
- `owned-serial-prototype.exe`: SHA256
  `88f4e7284052651e8c48b3ebac065cfbc2593dbeb3e57740be514852264e97ec`.
- `two-job-compact-prototype.exe`: SHA256
  `7dc3df444d2b38fa389320587582be2042523ef20bfca8f89cf320112d3c6948`.

Final native evidence в `out/fix553/{before,caller}-native{,-repeat}-120/`;
prototype — `after-native-120/`, `final-native-120/`. `summarize.py`
требует comparison gates, 3600 steps и совпадение полного gameplay
evidence; aggregate — `native-summary.json`, `native-summary.print.txt`.
Ранние fixed-order bench logs `bench-geometry*.log` сохраняются;
controlled — `bench-geometry-controlled.log`, final caller —
`bench-geometry-caller.log`. Final tests/build/Clippy —
`tests-render-caller.log`, `build-caller.log`, `clippy-caller.log`.
Before/prototype/final test binaries заморожены. Out не архив Git.
