# 334. Индекс опор с одним head вместо head/tail

Дата:2026-10-19. База:`0387466`. Единственная очередь —
[316](316-optimization-delivery-plan.md). Статус: **принят**. Повторный
headless performance gate и полный release qualification пройдены;
ordinary native evidence valid, FPS/realtime acceptance остаются открыты.
Алгоритмическое решение — ADR-751. Не persistent membership04a.

## Алгоритм

Временный индекс Settled делит клетки по X modulo8 между прежними workers.
Раньше каждая клетка хранила два usize: head и tail. Добавление источника
писало новый узел и исправляло successor прежнего tail.

Provider requests по-прежнему распределяются в source order. Внутри
каждой stripe новый builder проходит requests в обратном порядке,
добавляя источник перед прежним head. Поэтому готовый контактный список
по-прежнему перечисляет источники в исходном возрастающем порядке.
Обход клеток внутри запроса, точные bounds, query mask, проверки поддержки
и порядок читателей не меняются. Hash-итерация не участвует в результате.

На клетку остаётся один usize вместо двух. Запись в предыдущий узел
не нужна: successor нового узла сразу указывает на прежний head.
Нулевой successor завершает список; u32::MAX остаётся sentinel для
полного usize successor в wide map. Уплотнение не вводит лимит индекса
и не меняет source u32. Wide fallback тестируется малым порогом.

Меняется только производный arena layout текущей фазы. Он не входит
в snapshot/hash. Полный rebuild, текущая геометрия, сроки и квоты прежние;
стабильные IDs/dirty membership04a этим не реализованы. Уменьшение value
16→8 bytes не выдаётся за измеренное уменьшение RSS или всех allocations.

## Предварительные проверки

Debug:10 Settled-тестов и kernel oracle для parallel wake, движущихся
корпусов, reservations и restore прошли. Контактные списки сравниваются
с независимым BTreeMap cell-by-cell reference; отрицательные координаты,
перекрытия, serial/parallel,1/2/4/8 workers, очистка и wide links покрыты.
Полный release lib:1011 passed,16 ignored,0 failed; compile4m15s,
tests213,71s. Architecture12/docs3 прошли; fmt и diff check чистые.
Итоговый executable и ordinary native проверены после headless приёмки.

## Первые ordinary контроли

World6000, physics4/navigation8/Rayon4. before-a явно скопирован из333:
это тот же исходный запуск332, не новая независимая пара.
Порядок запусков: before-a → неудачный333 → after-a334 → after-b334 →
before-b332 → before-c332 → after-c334. Компиляция и проверки выполняются
отдельно от замеров.

| Run | Root mean |p99|max|Late root|Late Settled|Late cells, мс |
|---|---:|---:|---:|---:|---:|---:|
| before-a |16,70725|43,477|105,426|18,86162|2,77617|1,08817|
| after-a |17,00198|43,868|106,626|19,31776|2,75083|1,08308|
| after-b |16,77072|43,913|105,102|18,92009|2,66150|1,00800|
| before-b |17,05554|44,539|105,333|19,31270|2,86017|1,16483|
| before-c |16,88597|45,550|104,223|19,12966|2,79958|1,15500|
| after-c |16,61795|43,558|106,440|18,72966|2,58850|0,96250|

Все три сравнения сохраняют36 не-временных полей каждого тика и hash
`21bef878463f364c`. Во второй свежей паре cells дешевле13,5%, root1,7%.
Средний root первых двух before/after, напротив, практически равен:
16,88140→16,88635 мс. Первая пара не доказывает прирост полного боя.
Следующая прямая соседняя пара c подтверждает cells−16,7%, root−1,6%.
Среднее **двух свежих соседних пар b/c**, включая обратный порядок b:
root16,97076→16,69434 мс (−1,63%), late cells1,15992→0,98525 (−15,1%),
Settled2,82988→2,62500 (−7,2%), debris5,49210→5,25852 (−4,3%).
Если включить раннюю пару a, mean root16,88292→16,79688 (−0,51%).
Первая пара не замалчивается: разброс root заметен и unrelated rubble/motion
тоже меняются. Выигрыш целевого cells есть во всех трёх сравнениях,
но у a лишь0,5%; свежие b/c подтверждают пользу полного тика.
P99 в b/c ниже, max в c выше; every-tick gate и редкие пики не исправлены.
Это небольшое точное ускорение, а не15% всего движка. Выигрыш FPS пока
не заявлен. После probe менялся только комментарий builder, без runtime
изменений; final executable проверяется отдельно.

Доказательства: `out/optimization-334/`: before/after-a,b,c CSV/log,
summary.json/analyze.py, settled-tests.log/kernel-tests.log,
probe-build.log, source-inputs.json, probe-burst.rs/probe-bench_battle.exe.
Первый отклонённый подход — [333](333-equal-settled-request-runs.md).

## Итоговая сборка и дополнительные сценарии

Locked/offline release build трёх программ завершился за2m37s. После
probe изменён только комментарий; финальный binary отдельно квалифицирован
6000 шагами. Все36 полей совпали с before-c; hash `21bef878463f364c`.
Mean root16,43568, p9946,428,max107,122 мс; late root18,51667,
debris5,14665,rubble5,08620,motion4,75619,cells0,99750 мс.
Это одиночный контроль итогового файла, не дополнительная независимая A/B.
Более высокий p99 ещё раз показывает, что хвосты этим срезом не исправлены.

Пять парных ordinary deployment AI боёв; **не MissionRun и не все26 сцен**:

| Сцена / тики | Root before→after, мс | Hash |
|---|---:|---|
|4 /900|1,74363→1,74044|39ed0e187213fa88|
|5 /900|2,25995→2,26570|1fd7abba7eec5963|
|12 /4000|3,31521→3,33132|354e0b04fed1bec1|
|15 /4000|1,81091→1,81590|4eb06a9e3e77a598|
|16 /4000|2,09258→2,07494|3edaedcda5bda080|

Все36 игровых полей каждого шага/hash равны; root изменился от−0,84%
до+0,49%. Значимого выигрыша этих небольших сцен не заявлено.
Golden Battle600 на physics1/4/8: `635200a9c3fc1530` во всех случаях.

## Ordinary native240 в развёрнутом окне

Фактический контекст3840×1949,focus=true,scale1,AutoNoVsync,Windowed,
view1/zone6,overlays[true,true],drawer0,static camera;
physics4/navigation8/Rayon4/compute4. Capture отдельный, без одновременной
сборки/тестов/анализа. Analyzer `--require-valid` прошёл:
valid_for_comparison=true,acceptance_eligible=false,meets_simulation_rate=false.

- 40,2 FPS; p9541,589,p9956,249,max125,043 мс.
- 6948 SIM steps,ratio0,96465019; core19,87521 мс/step.
- Final debt28,999999 ticks; dropped223,771047 tick-equivalents.
- Hash `0850a0f84e748697` на6948 шагах.

Это **одиночный after**, не matched FPS A/B. Разные terminal hashes
запусков с разным количеством шагов не являются проверкой deterministic
равенства. Прирост FPS не доказан.30Гц без списания времени,165FPS/
every-frame≤6,06мс,headless every-tick≤33мс и редкие длинные фризы остаются
открыты. Максимум этого headless контроля107мс, native125мс не доказывает
отсутствия секундного stall вне240s окна.

## Воспроизведение и provenance

База executable: `out/optimization-332/after-bench_battle.exe`.
Candidate: `out/optimization-334/probe-bench_battle.exe`; final:
`out/optimization-334/after-bench_battle.exe`. Ordinary команда для каждого:

```powershell
$env:RAYON_NUM_THREADS='4'
& $binary --world --ai-vs-ai --ticks 6000 --physics-workers 4 --workers 8 --tick-csv $csv
```

Native:

```powershell
& ./scripts/profile-range.ps1 -Seconds 240 -Scenarios world-battle -Output out/optimization-334/native-after240 -SkipBuild -BinaryPath out/optimization-334/after-blast.exe -PhysicsWorkers 4 -NavigationWorkers 8 -RayonWorkers 4 -ComputeWorkers 4 -Maximized
```

497 source/data fingerprints проверены вокруг final build; относительно332
меняется только `crates/sim-core/src/burst.rs`. Build sidecars относятся
к WIP поверх0387466, не доказывают clean-build/toolchain attestation.
Frozen SHA256:

| Программа | SHA256 |
|---|---|
|bench_battle|d0c03e25415684a35edc47e194d7ba4bb2e2ad9af27cc645db77167e07c2e5f9|
|sim-cli|b1bf9ac7236e1e0f824e7dd166cc063c38fef0addd6f12a65e9b527a90c16b1d|
|blast|7fdb1af9494003b716651cacdbb3899fd19001cb22172117351874b6d4ff9ed5|

Production/scenario summaries, full tests/build/golden logs, native manifest,
raw CSV/log и analyzer report находятся в `out/optimization-334/`;
`artifacts.json` фиксирует файлы evidence. Прототип333 заморожен отдельно.
