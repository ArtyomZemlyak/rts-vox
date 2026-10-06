# ADR-600 — диагностика камеры и сохранение actor cache

Дата:2026-10-06. Исходный main102a2bd. Артефакты:out/fix600.
Продолжение ADR-599, WBPERF-009/027.

## Что мешало понять задержки

camera.csv показывал лишь parent stream_ms и progress. Пустой progress
не отличал small shift от публикации staged window. На пиках ADR-599
нельзя было доказать, тормозит allocation, rows, publication или occupancy.
Оценка small shifts по delta origin была только приближённой выборкой.

Camera work schema1 добавляет явные idle/map/shift/staging/publish,
frame_row/generation и7эксклюзивных same-thread wall фаз: allocate, rows,
publish, retire, occupancy, advance, shift. Измерение не делает I/O в
кадре; CSV и отчёты пишутся после capture. Rows содержит dispatch/join;
это не CPU workers или чистое ожидание. Все leaves ниже parent; unscoped
remainder считается отдельно. Timers включены только для camera tour.

Analyzer проверяет schema announcement, finite/nonnegative timings,
сумму leaves≤parent с допуском округления0.01мс, row index и tick/generation
с main CSV. Невалидная привязка исключает сравнение. Legacy tour schema1
без work schema читается прежним способом. Peak JSON содержит camera work
previous Update и current Update раздельно; Markdown объясняет предыдущий.
Observed begin-to-begin interval включает previous Update и следующий
gap, поэтому текущие leaves нельзя выдавать за причину предыдущего кадра.

## Найденная причина и фикс

Instrumented before: пик stream15.7947мс на19.755s, publish operation.
Из него rows0.5105, publication1.4512, retirement0.5348,
occupancy13.2952мс. Для14публикаций mean occupancy5.5867мс против
publication1.2419мс. Allocation за tour max0.0207мс: в этом повторе
выделение нового окна не объясняет пик. Время в stages — wall, не CPU.

Публикация делала `*visual=default()`: теряла геометрию всех actors,
кэш world cells, scratch allocations и историю анимации. Следующий
cached_cells_in считал roster новым и преобразовывал voxel bodies всей
армии, включая тела за границами окна. На обычном обновлении они уже
отсеивались по bounds; начальная сборка cache этот отсев обходила.

moving::replace_terrain очищает только applied occupied cells и window
marker. Следующее occupancy обязательно наносит все текущие shadows на
raw terrain, даже при том же origin; actor caches и история остаются.
Scene replacement по-прежнему вызывает moving::reset и удаляет actor
cache до публикации. Начальный new_occupied использует тот же
консервативный bounds predicate, что update_occupied; скрытые тела
перечисляются при реальном входе с актуальными pose/damage/mount keys.
Детализация, shadow materials, GPU patch contents и core не упрощены.

## Проверки и происхождение

Рабочее дерево:124headless render tests passed,3manual ignored,
13HUD tests filtered (UI не менялся).125Python analyzer tests проходят.
До фикса diagnostic вариант также прошёл123headless и125Python tests.
Новый тест сравнивает fresh volume и replay GPU patches: same origin,
wrap/shift, пустое окно, damage/aim при скрытии и scene reset. Требует
повторного нанесения теней, сохранения geometry allocations и pose history.
Прежние pooled/serial occupancy и staged rows tests сохранены.

Изолированный экспорт индекса без исходных WIP:4streaming,22moving и
14profile tests проходят,1manual occupancy benchmark ignored;
125Python tests также проходят. Четыре Rust source совпадают побайтно
между staged index, tested export и frozen after; Python sources совпадают
между index/export/worktree. Целевой release полигон пересобран.

Before.exe SHA664737740828D0F0D2969EE3E7846182C786DB798B120E3FB509D4F5475D6087;
after.exe SHA7E2AE69A770930E60A8DF8F32E331E693A8D57F27ADB1FE592E0D95D7737C944.
Оба RefreshTrackedInputs,355inputs, одинаковый recorded toolchain.
Все hashes проверены: между instrumented B/A отличаются только moving.rs
и streaming.rs. Profile/camera instrumentation в обеих сборках одинакова.
Исходные WIP входят в обе native сборки, остаются вне коммита.
SourceAtLaunch сам по себе не заменяет эту проверку. Сборки и тесты не
конкурируют с native профилем; видео/readback пикселей нет.

## Native camera tour: B→A1→A2

60s world battle,60actors,AI обеих сторон,physics/navigation8,compute4,
RTX5090 Vulkan,1600×900 AutoNoVsync. Все три run валидны/AcceptanceEligible,
обе пары проходят matched_recorded comparison gate. Каждый1800ticks,
tempo1×,hash1f697f865e6f8aa6; alive/shots/damage совпадают,0blocked frames.

| Метрика, мс если не указано иначе | B | A1 | A2 |
|---|---:|---:|---:|
| Полный кадр mean |9.4549|9.3292|9.5007|
| FPS:1000/mean |105.8|107.2|105.3|
| Полный кадр p99 |27.9242|26.9364|27.6821|
| Полный кадр max |35.3570|37.5379|36.3550|
| Core/step |11.4295|11.3014|11.3902|
| Publish operation count |14|14|13|
| Publish operation mean |8.6112|4.4489|4.7940|
| Publish operation max |15.7947|12.5193|12.3183|
| Nested occupancy mean |5.5867|2.5145|2.8193|
| Nested occupancy max |13.2952|10.3482|10.1381|
| Nested retirement mean |1.1611|0.1750|0.1695|
| Nested retirement max |3.5369|0.2102|0.2555|
| Nested publication mean |1.2419|1.2189|1.2802|

Publish operation включает последние rows и остальные leaves; его не
складывать с ними. Mean operation −44–48%,occupancy −50–55%,retirement
−85%. Сам transfer/publication почти прежний. Wall tour независим от
симуляции; разная cadence меняет моменты/число публикаций, так что это
сравнение сценария, а не тождественный microbenchmark input.

Средний FPS gain не устойчив: A2 ниже B. Полный max кадра хуже в обоих
повторах. Поэтому fix не объявляет исправленными rare stalls или165FPS.
Пик occupancy всё ещё≈10мс; synchronous core≈11.4мс.

## Следующий приоритет

WBPERF-009: разобрать occupancy geometry dispatch/join, union и material
apply отдельно; сохранять exact raw/shadow patch semantics. Rows staging
имеет редкие пики, тоже требует leaf разбора до нового распараллеливания.
WBPERF-002: перенос авторитетного tick/publication с main-thread boundary
остаётся основным ограничением всего кадра. COW/strip join пока не имеют
отдельных scopes; shift_ms включает writer целиком. WBPERF-027 частично,
165FPS, многосекундные зависания и физический P-87 открыты.
