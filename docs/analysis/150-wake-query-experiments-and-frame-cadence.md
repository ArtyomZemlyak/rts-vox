# ADR-556 — проверка параллельных кандидатов и распределение кадров

04.10.2026, исходный HEAD `5ea41c2`, WBPERF-001/004/005/014/027.
Артефакты: `out/fix556`, frozen executables/build manifests, tick CSV,
`core-summary.json`, `native-summary.json`, обычные native datasets,
сохранённые исходники трёх отклонённых прототипов и test logs.
Измерения последовательные, без сборок, тестов, правок или тяжёлого
анализа во время замера. **Ни один новый алгоритм физики этой серии
не оставлен в штатном коде.** Принято улучшение offline-профилирования.

## Что проверено и отклонено

1. Принудительный `with_max_len(1)` для classify/gather пакетов ADR-555:
   rubble 1,038851→1,041485 мс, полный tick 13,078736→13,168858 мс.
   Дополнительное дробление не дало выигрыша. Прототип удалён.
2. Плотная membership mask и parallel stacked-support queries пакетами
   по 256 от 1024 входов: первая пара rubble 1,038851→1,065637 мс,
   обратная 1,049854→1,037844 мс. Устойчивого выигрыша нет; первый
   rubble max 35,786 мс сохранён, причина не установлена. Полный tick
   13,078736→13,646262 / 13,266898→13,232066 мс. Прототип удалён.
3. Пространственный индекс hulls для существующих parallel wake queries.
   Ячейка 256, membership-only lookup, source-ordered 64-bit candidates,
   прежние exact voxel checks. Fresh immutable index на фазу, включение
   от 1024 fragments и восьми hulls. Более 64 hulls, чрезмерные bounds
   или запросы возвращаются к старому поиску; лимит 256 ячеек на range.
   Новых pools, сохранённого кеша или изменения commit order нет.
   CPU локально ускорился, **обычный native бой заметно замедлился**.
   Прототип и его test-only helper сохранены в `grid-burst.rs`, из
   production/test source удалены; baseline burst восстановлен побайтно.

Все завершённые CPU runs сделали 3600 ticks полного боя 30 на 30,
AI обеих сторон, physics/nav=8. Every-tick counters, final state,
per-unit evidence и hash `bcc1f58c226d1721` совпали. Прототип индекса
прошёл 562 core tests, четыре прежних ignored; новый differential test
покрывал negative/fractional coordinates, cell boundaries, повороты,
voxel holes, source-first overlap, bit 63 и bounded fallbacks. Clippy
lib/tests не добавил предупреждений. Корректность не означала ускорения.

## CPU индекс — воспроизводимое локальное ускорение

Порядок before→grid→grid→before, 3600 ticks каждого run. Mean wake
получен из 12 округлённых окон по 300 ticks, ошибка округления менее
1 мкс. Построение индекса включено в wake phase, а не вынесено из замера.

| Метрика | До / 1 | Grid / 1 | До / 2 | Grid / 2 |
|---|---:|---:|---:|---:|
| Mean wake, мкс | 630,083 | 518,667 | 638,250 | 521,083 |
| Mean tick, мс | 13,078736 | 12,968426 | 13,176391 | 12,980310 |
| Mean debris, мс | 3,116873 | 3,044446 | 3,157181 | 3,022275 |
| Tick p99, мс | 21,293 | 21,493 | 21,928 | 21,269 |
| Tick max, мс | 48,841 | 49,184 | 50,541 | 51,475 |
| Debris max, мс | 9,788 | 16,456 | 6,572 | 6,227 |

Wake −17,68% / −18,36%, полный tick −0,84% / −1,49%. Общие differences
не целиком атрибутируются wake: неизменённые фазы тоже меняют длительность.
Пики не улучшены устойчиво, устранение stalls не подтверждено.

## Ordinary native — кандидат отклонён

Before→grid→grid→before, четыре runs по 120 с, 1600×900 AutoNoVsync,
physics/nav=8, compute=4, штатные эффекты, detailed observers выключены.
Все условия manifest/offline pair gates проходят. Каждый run делает
3600 steps при темпе 1×; полный gameplay evidence и hash прежние.
Eligibility означает сравнимые условия, **не speed acceptance**.

| Метрика | До / 1 | Grid / 1 | До / 2 | Grid / 2 |
|---|---:|---:|---:|---:|
| FPS | 81,4206 | 69,0573 | 81,5248 | 69,1421 |
| Mean frame, мс | 12,281908 | 14,480731 | 12,266209 | 14,462964 |
| p99 frame, мс | 31,5737 | 31,9064 | 31,4917 | 31,7181 |
| Max frame, мс | 63,9246 | 64,3094 | 65,4554 | 65,7204 |
| Core/step, мс | 14,505287 | 14,590337 | 14,515003 | 14,599083 |

FPS примерно −15,2% в обеих парах, core/step около +0,6%. Дополнительная
свежая сборка прежнего burst с forced source refresh дала **85,1165 FPS**,
mean frame 11,748597 мс, p99 30,8928 мс, max 63,3319 мс, core/step
14,258560 мс; 3600 steps, тот же gameplay/hash. Rebuild control не
объяснил просадку старым frozen executable. Это дополнительный single run,
не доказательство точной причины или оценки её величины. Grid отклонён
по native результату; локальные −18% не основание оставить регрессию.

## Принято: frame cadence в offline analyzer

Прежние slow-frame/slow-slice записи показывали отдельные дорогие rows,
но не распределение всего capture по кадрам с нулём/одним/несколькими
шагами. Новое поле JSON `frame_cadence` и таблица Markdown покрывают все
полные begin-to-begin intervals, группируя по **предыдущей Update slice**
и её generation/step count. Следующий gap принадлежит этой slice, а не
текущему core. Длительности slice/gap считаются непосредственно по
монотонным ns boundaries, интервалы включены ровно один раз.

Сохраняются count, completed steps, mean/p99/max интервала, Update,
следующего gap и nested core; wall totals/fractions, generation crossings.
Последняя Update явно terminal: не получает выдуманного следующего gap.
Поэтому в этих runs cadence содержит 3599 steps, а полный профиль — 3600.
Legacy без clocks получает null. Multi-step core остаётся стоимостью
slice, не выдуманными отдельными tick durations. Новых runtime clocks,
buffers, native observers или scheduling changes нет.

Точность привязки меняет вывод: предварительная группировка gap по
текущему row ошибочно локализовала разницу перед кадрами с тиком.
Правильная группировка показала её **после предыдущих zero-step slices**:

| Предыдущая slice | До / 1 | Grid / 1 | До / 2 | Grid / 2 |
|---|---:|---:|---:|---:|
| Zero-step intervals, count | 6172 | 4688 | 6185 | 4698 |
| Mean zero-step Update, мс | 1,1680 | 1,2429 | 1,1789 | 1,2590 |
| Mean following zero-step gap, мс | 4,4528 | 6,0715 | 4,4271 | 6,0362 |
| Mean one-step Update, мс | 19,8701 | 19,9799 | 19,8881 | 20,0001 |
| Mean following one-step gap, мс | 3,8350 | 3,8356 | 3,8238 | 3,8193 |

Это локализация wall-различия, **не GPU time, lock wait или доказанная
причина**. Update/core вложены; их нельзя прибавлять к gap как отдельные
CPU totals. OS process equivalents около 2,95…2,96 до / 3,12…3,18 у grid
не доказывают балансировку. Detailed render/source spans, OS stacks/waits
и GPU attribution этих ordinary runs отсутствуют.

## Provenance, проверки и дальнейший приоритет

ADR-554 refresh/hash/freeze protocol применён ко всем новым builds.
Baseline burst SHA `FB7F0BC196C2041E92CB2329AB52EFED7CFF1E259D8A77AF81BA8025B2AF88C6`,
grid source SHA `6DE26FF90E4BA62918607A976761A6D6E4C2321C253558F9D591C2A242458B8A`.
Reuse старых verified ADR-555 binaries ограничен их listed source evidence,
не полной dependency/clean-checkout аттестацией. SourceAtLaunch и GitHead
сами по себе не удостоверяют executable. Все прежние 13 WIP entries сохранены.

- CPU before: `9db3141f90c3fdff2632b1cae64fbc265de1a4f7a0b39a10dc07e3430a8bec69`.
- CPU grid: `5e7728893180035f78b3b6daa28b1b5cc33ec3c5c47b563884a2f37178416a2c`.
- Native reused control: `aa8b674b8417a13f89ceaddf805a4ab8d351b9eb93ed9b4f094cea5cd6516193`.
- Native grid: `a8731bb9f667efc53e15277f077685014df7872e014e4eeba699c8f71198d67e`.
- Fresh control/current `target/release/blast.exe`:
  `3c1ed8b7f9513ffba14d5aea7b598a858151481a8061e6abef24b7682acbe03c`.

Offline analyzer: **102 tests passed**, включая семь новых cadence tests
и интеграционную проверку Markdown/JSON. Все пять native datasets повторно
прочитаны новым analyzer и проходят evidence gates. Exact restored core:
**561 passed, 4 прежних ignored**, 112,77 с в отдельном final release suite.
Diff checks проходят.

Повторное чтение готового capture, без запуска игры:

```powershell
python scripts/analyze-range-profile.py out/fix556/grid-native-120 --compare out/fix556/before-native-120 --require-valid
```

Новая таблица находится в `world-battle.analysis.md`, machine-readable
partition — в `world-battle.analysis.json`. Эти отчёты доступны и для
прежних clocked captures; пересъёмка не нужна.

WBPERF-001/027 частично: теперь видно общее распределение frame wall,
а не только самые медленные кадры. WBPERF-004/005/014 не закрываются
неудачными прототипами. Следующий приоритет — render/system/OS attribution
zero-step gaps и WBPERF-002: независимая owned simulation с compact read
model, без полного clone мира. Balanced CPU, причина native просадки,
редкие stalls и 165 FPS остаются открыты.
