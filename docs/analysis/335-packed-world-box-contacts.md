# 335. Точный объёмный запрос битовой занятости для обломков

Дата:2026-10-19. База:`main 73d329e`; единственная очередь —
[316](316-optimization-delivery-plan.md). Статус: **принят**. Повторный
ordinary performance gate и полный release qualification пройдены;
native evidence valid, FPS/realtime acceptance остаются открыты.
Алгоритмическое решение — ADR-752.

## Выбранный расход

После334 поздний World по-прежнему тратит около5мс на debris,5мс на rubble
и4,8мс на motion. Обломки и классификация карты завалов вызывают общий
`burst::box_contact`: broad phase и проверки отдельных частей формы читают
неизменную занятость грунта. Старый путь вычисляет границы строк и вызывает
`World::solid_bits` для каждого Y/Z ряда и каждого64-воксельного X отрезка.

Кандидат — общий `World::any_solid_box(lo,hi)`, не сценарная ветка.
Inclusive u32 границы обрезаются картой; всё снаружи мира — воздух,
перевёрнутый диапазон пуст. Метод непосредственно читает нынешний bits,
не материал, генератор или сохранённый ответ. Правки того же тика видны.

Маски первого/последнего X слова вычисляются один раз для всего объёма.
Один X word получает отдельный путь; широкая коробка проверяет head,
полные внутренние words и tail. Обход Y/Z идёт в порядке backing volume,
Z-строки читаются с постоянным stride. Новый метод не выделяет heap,
не добавляет кэш, persistent состояние или новые игровые ограничения.

`box_contact` сохраняет прежнее преобразование Fx→floor integer,
непроницаемые боковые/нижнюю границы карты и отсутствие верхнего потолка.
После этих правил общий World запрос читает тот же набор целых вокселей.
Порядок раннего boolean выхода может отличаться; чтение не имеет эффектов,
логических квот или источников ничьей. Narrow phase `fragment_contact`
сохраняет каждую занятую часть формы; пустые углы масок не заполняются.
Материя, взрывы, evict, сроки, SIM frequency и snapshot/hash прежние.
Это не persistent membership04a и не изменение физической модели.

## Адресные проверки

- Два voxel debug теста прошли: повоксельный oracle на sparse worlds
  шириной1/63/64/65/129/150,12000 случайных коробок, границы слов/мира,
  инверсии, предельные u32 endpoints; add/remove в одном тике.
- Sim debug test сравнил новую физическую обёртку с прежними Z/Y/X loops:
  4000 дробных поз, обычные/нулевые/расширенные переданные bounds и
  перевёрнутые local bounds. Это24000 независимых сравнений ответов.
- После этих debug запусков добавлен только `#[inline]` к новому методу;
  финальный source/release прошёл отдельную полную проверку ниже.

## Первая ordinary пара World6000

physics4/navigation8/Rayon4, stock deployment AIvsAI. Последовательно:
frozen334 before-a → debug/build отдельно → probe335 after-a.
Все36 не-временных полей каждого тика/hash `21bef878463f364c` совпали.

| Метрика, мс | Before-a | After-a |
|---|---:|---:|
| Root mean |16,39029|16,30007|
| Root p99 |45,107|43,254|
| Root max |106,474|102,627|
| Late root |18,47653|18,37060|
| Late debris |5,13367|5,12200|
| Late rubble |5,05254|4,79962|
| Late translate |1,25467|1,12158|
| Late wake |0,36792|0,37583|
| Late Settled |2,58200|2,65925|

Translate дешевле10,6%,rubble5,0%,root0,55%. Wake не ускорился,
неизменённый Settled также колеблется: эффект нельзя переносить на все
фазы или выдавать за устранение пиков.

## Повтор в обратном порядке

Соседние after-b335→before-b334, те же настройки,6000 шагов каждый.
Снова совпали36 игровых полей каждого тика и hash `21bef878463f364c`.

| Метрика, мс | Before-b | After-b |
|---|---:|---:|
| Root mean |16,64455|16,20541|
| Root p99 |43,569|43,560|
| Root max |104,939|103,926|
| Late root |18,77113|18,17769|
| Late debris |5,26333|5,08123|
| Late rubble |5,18411|4,76007|
| Late translate |1,28467|1,11667|
| Late wake |0,37500|0,37508|
| Late Settled |2,64550|2,62125|

Среднее двух before/after:root16,51742→16,25274мс (−1,60%),
late root18,62383→18,27415 (−1,88%),rubble5,11832→4,77985 (−6,61%),
translate1,26967→1,11913 (−11,86%),debris5,19850→5,10162 (−1,86%).
Это выигрыш двух родителей и полного тика, а не11,9% всего движка.
Wake не ускорился; p99 второй пары почти равен. Every-tick≤33мс и редкие
пики не исправлены. Ordinary native проверяется отдельно.

Перед полной приёмкой усилен только fixture voxel oracle: отдельные
единственные попадания в последнем частичном X word и последней Y/Z
строке не маскируются ранним head hit. Runtime после probe не менялся.
Full release: sim-core1012 passed,16 ignored,0 failed (215,10s);
voxel173 passed,0 failed (0,54s), compile4m22s. Architecture12/docs3,
fmt и diff check прошли. После тестов исправлен только комментарий fixture;
runtime исходник остаётся равен probe. Итоговый build и native проверены
после этой headless приёмки.

Evidence:`out/optimization-335/`:source-inputs.json, frozen probe binary,
probe-burst.rs/probe-world.rs, CSV/log/analyze.py/summary.json,
box-tests.log/contact-tests.log, probe-build.log. Runtime changes ограничены
`crates/voxel/src/world.rs` и `crates/sim-core/src/burst.rs`.

## Итоговая сборка и пять дополнительных сцен

Locked/offline release build bench_battle/sim-cli/blast:2m44s. Отдельный
ordinary6000 итогового executable:все36 полей совпали с before-b,
hash `21bef878463f364c`. Root mean16,23170,p9943,840,max105,326мс;
late root18,25398,debris5,11705,rubble4,76739,motion4,77295,
translate1,11650мс. Это qualification финального файла, не дополнительная
независимая A/B пара. Runtime равен probe; различия исходника только
в voxel test fixture/комментарии.

Парные ordinary deployment AIvsAI бои, **не MissionRun и не все26 сцен**:

| Сцена / тики | Root before→after, мс | Hash |
|---|---:|---|
|4 /900|1,75352→1,70092|39ed0e187213fa88|
|5 /900|2,26403→2,19859|1fd7abba7eec5963|
|12 /4000|3,36374→3,25859|354e0b04fed1bec1|
|15 /4000|1,83385→1,77216|4eb06a9e3e77a598|
|16 /4000|2,10658→2,04441|3edaedcda5bda080|

Все36 игровых полей каждого тика и hash равны. Root дешевле2,89–3,36%.
Golden Battle600 на physics1/4/8: `635200a9c3fc1530` во всех случаях.

## Ordinary native240 в развёрнутом окне

Actual3840×1949,focus=true,scale1,AutoNoVsync,Windowed,view1/zone6,
overlays[true,true],drawer0,static camera;physics4/navigation8/Rayon4/compute4.
Во время capture не выполнялись сборки, тесты, анализ и форматирование.
Analyzer `--require-valid` прошёл:valid_for_comparison=true,
acceptance_eligible=false,meets_simulation_rate=false.

- 40,4 FPS;p9541,992,p9956,117,max125,240мс.
- 6950 SIM steps,ratio0,96486240;core19,34279мс/step.
- Final debt28,999999 ticks;dropped222,221975 tick-equivalents.
- State hash `3905f464db16e8d7` на6950 шагах.

Это **одиночный after**, не matched FPS A/B. Native terminal hashes разных
запусков с разным количеством шагов не сравниваются как deterministic
равенство. Прирост FPS не доказан. Экономия SIM не означает достижение
реального темпа30Гц: этот gate по-прежнему не пройден.165FPS/every-frame
≤6,06мс,headless every-tick≤33мс и редкие секундные stalls остаются открыты.
Максимум headless105мс/native125мс не исключает stall за пределами240s.
Причину оставшегося frame/pacing ограничения этот срез не устанавливает.

## Воспроизведение и provenance

Baseline:`out/optimization-334/after-bench_battle.exe`;probe/final:
`out/optimization-335/probe-bench_battle.exe` / `after-bench_battle.exe`.

```powershell
$env:RAYON_NUM_THREADS='4'
& $binary --world --ai-vs-ai --ticks 6000 --physics-workers 4 --workers 8 --tick-csv $csv
& ./scripts/profile-range.ps1 -Seconds 240 -Scenarios world-battle -Output out/optimization-335/native-after240 -SkipBuild -BinaryPath out/optimization-335/after-blast.exe -PhysicsWorkers 4 -NavigationWorkers 8 -RayonWorkers 4 -ComputeWorkers 4 -Maximized
```

497 source/data fingerprints проверены вокруг final build. Относительно334
меняются только два указанных Rust файла. Probe/test/final source manifests
сохранены отдельно; runtime prefix World до fill_tests равен probe,
burst.rs равен полностью. Sidecars относятся к WIP поверх73d329e,
не являются clean-build/toolchain attestation. Frozen SHA256:

| Программа | SHA256 |
|---|---|
|bench_battle|0b9760799cb7246e8ab0dad9e2c22e44d986bc7aceb2de77585ae56778cc3d28|
|sim-cli|7be8ae70735ddda5f6850c2630fd090692cee7b9d2655fc28ef254a3be5ea093|
|blast|b12387fc0ea6ce0a97f85d8b55820f1b3a45c51e93b825238c522fc30bd9e24a|

Production/scenario summaries, full tests/build/golden logs, raw native
CSV/log/manifest и analyzer report лежат в `out/optimization-335/`.
`evidence-verification.json` проверяет source/binary соответствие;
`artifacts.json` фиксирует файлы evidence. Снижение RSS/VRAM не заявлено.
