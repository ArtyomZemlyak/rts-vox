# 332. Точный расчёт ключей слияния куч без runtime делений

Дата:2026-10-19. Runtime база:`12300a8`, документационный checkpoint:
`9db5915`. Единственная очередь — [316](316-optimization-delivery-plan.md).
Статус: принят узкий точный арифметический срез, ADR-750; общие budgets
и FPS acceptance остаются открыты.

## Расход и точное преобразование

В позднем World ключи resting piles занимают около0,3 мс. Для каждого
обычного resting фрагмента строятся три координаты через
`to_int_trunc().div_euclid(pile_cell as i32)`. Параметр читается из данных;
stock pile_cell16, но общий путь каждый раз выполняет runtime division.

У `Fx::to_int_trunc` историческое имя: реализация — арифметический сдвиг
на FRAC_BITS, то есть floor, включая отрицательные дроби. Для положительного
целого d=2^k исходный floor(floor(q)/d) равен floor(q/d). Поэтому новый
`PileGrid` один раз за фазу распознаёт положительную степень двойки и
использует raw >> (FRAC_BITS+k). Это одна точная арифметическая операция
на координату вместо преобразования и Euclidean division.

Для остальных значений сохранён исходный путь. В частности, положительный
u32, который после прежнего cast становится неположительным i32,
не попадает в быстрый путь. Нулевой divisor сохраняет прежнее поведение;
авторские данные уже отвергают pile_cell0. Максимальный fast shift62.
Полное покрытие signed Fx raw диапазона не требует clamp или округления.

Ключи, hash/shard выбор, source order head/donor, переносы материи,
restack/seat и физика прежние. Радиус/сила оружия, опоры/навигация,
SIM frequency/quotas и snapshot/hash не меняются. Не добавлены кэш,
heap allocation, состояние по фрагментам или новые игровые ограничения.

## Проверки перед performance gate

Differential test проверяет все положительные i32 степени двойки,
несколько иных размеров и большие u32 cast cases, целые/дробные значения
обоих знаков, i64 MIN/MAX и соседние raw значения. Reference — прежняя
формула с div_euclid. Явный случай−16,125 и−0,125 при cell16 даёт−2/−1:
это floor, а не truncation toward zero. Публичная Fx API не переименована.
Kernel snapshot oracle с настоящими переносами/claim и restore прошёл.

[328](328-compact-fragment-landings.md), [329](329-compact-settled-endpoints.md),
[330](330-complete-hull-skip-mask.md), [331](331-rigid-fragment-contact-geometry.md)
не приняты и отсутствуют в runtime. Их адресные debug checks не выдаются
за full release/native этого варианта.

Стенд: frozen327 baseline, ordinary World AIvsAI6000,physics4/nav8/Rayon4.
Before-a без изменения взят из328 (тот же baseline binary/параметры),
не является свежей соседней парой. Требуется reverse контроль.
Source-inputs и raw — `out/optimization-332/`.


## Первый probe и вариант встраивания

В первом варианте root16,93089→16,81122 мс, late keys0,27158→0,30642:
сокращение формулы само по себе не ускорило целевой участок. Этот вариант
сохранён отдельно (`initial-*`), не подменён поздним результатом.

После явного inline(always) короткого `PileGrid::at`: root16,99217,
p9943,599,max106,093 мс; late root19,20704,debris5,45092,keys0,21592 мс.
Keys дешевле≈20,5%, но root относительно раннего before-a дороже≈0,36%.
Это ещё не доказанный выигрыш полного боя. Асимметрия may-inline/always-inline
наблюдается в стоимости; наличие конкретного машинного call не доказано.
Попытка прочитать COFF symbols не дала такого доказательства.

Оба after совпали с baseline по36 non-timing полям6000 шагов и hash
`21bef878463f364c`. Свежая before-b→after-b пара ещё проверяется. Подробные
фазовые времена — вложенные интервалы, не независимые дополнительные
расходы к root/debris.


## Две обычные пары6000

| Прогон | Mean root | P99 | Max | Late root | Late debris | Late keys |
|---|---:|---:|---:|---:|---:|---:|
|before-a|16.93089|44.001|105.443|19.14666|5.49717|0.27158|
|after-a|16.99217|43.599|106.093|19.20704|5.45092|0.21592|
|before-b|17.03021|46.029|105.137|19.29053|5.53738|0.27883|
|after-b|16.88613|43.802|105.861|19.08685|5.43330|0.21608|

Все времена в мс. Late2401..6000. Во второй **свежей** паре root−0,85%,
late root−1,06%,debris−1,88%,keys−22,5%. В первой пары root не дешевле,
keys−20,5%. Средние двух before/after: root16,98055→16,93915 (−0,24%).
Это маленький локальный выигрыш, не ускорение всего движка на20%.
Устойчивое снижение P99/max не доказано. Все36 игровых полей и hash
`21bef878463f364c` одинаковы в каждом6000-шаговом прогоне.

Корректность прежнего слияния проверяет также самостоятельный serial
reference в `parallel_pile_grouping_matches_ordered_merging_and_ledger`:
он сохраняет оригинальный div_euclid и сравнивает donor/head, массу,
контейнеры, геометрию и wrecks. Переносов материи не пропущено.


## Полная проверка исходника

Release SIM1011 passed,0 failed,16 ignored,213,48 с; сборка тестов4м16с.
Architecture12/12,docs3/3. Проверены347 Markdown файлов и1291 ссылка,
новых ошибок0; общие35 текстов и73 исторические замены дублей сохранены.
Runtime fingerprints497: от327 изменён только `burst.rs`.
Полный старый merge reference входит в прошедший набор.

Серия не квалифицирует5000 реальных юнитов,все26 сценариев,полный MissionRun,
долгий memory/VRAM soak или редкие секундные фризы. Не-показательные
локальные выигрыши328–331 не включены в результат принятого варианта.


## Итоговые исполняемые файлы

Locked/offline build bench_battle/sim-cli/blast2м41с. Все497 inputs
проверены перед freeze и после квалификации. Source/data content
provenance — WIP fingerprints; это не clean-build/toolchain attestation.
Итоговый bench SHA отличается от probe, поэтому6000 шагов дополнительно
исполнены именно итоговым binary и сравнены с baseline.

Qualification after6000:mean root16,90287,p9944,291,max106,109 мс;
late root19,11605,debris5,41358,keys0,21508 мс. Совпали все36 игровых
полей каждого шага и hash `21bef878463f364c`. Это отдельный after для
квалификации итогового binary, не новая соседняя A/B пара и не замена
таблицы probe. Прежние20–23% относятся к keys, не к целому engine.
Golden600 на1/4/8 workers: `635200a9c3fc1530` во всех случаях.

## Пять дополнительных сцен, итоговая сборка

| ID | Тиков | Mean root before→after, мс | Hash |
|---|---:|---:|---|
|4|900|1.78763→1.76239|`39ed0e187213fa88`|
|5|900|2.28747→2.26705|`1fd7abba7eec5963`|
|12|4000|3.37144→3.35036|`354e0b04fed1bec1`|
|15|4000|1.84707→1.83741|`4eb06a9e3e77a598`|
|16|4000|2.12787→2.12045|`3edaedcda5bda080`|

Во всех случаях совпали36 non-timing полей каждого шага. Root ниже
примерно0,4–1,4% в этих парах. Это обычный deployment→battle, не полный
MissionRun и не квалификация всех сцен каталога.

## Ordinary native240

Развёрнутое окно3840×1949,focused=true,scale1,AutoNoVsync,Windowed,
view1/zone6,overlays[true,true],drawer0,камера неподвижна. Stock:
physics4/nav8/Rayon4/compute4. Другие тяжёлые проверки одновременно
не выполнялись. Подробных CPU/GPU/OS traces нет.

Анализатор --require-valid принят: evidence_valid=true,
fps_eligible=false,realtime=false. За240 секунд6917 шагов,
ratio0,9602891; mean40 FPS,p9541,916,p9957,162,max121,314 мс.
Core20,10966 мс/реальный шаг. Конечный debt≈29 тиков,
списанное pacing-время255,06655 tick-equivalents, без удаления SIM-шагов.
Конец:fragment_records=77185,felling_pending=6520904,
hash `a9f0dc173938a4cc` на6917.

Это одиночный after; старые native327 имеют другое число шагов и время
сессии. **Прирост FPS причинно не доказан.** Новый контекст не смешивается
с headless fixed-work gain. 30Гц без списания pacing-времени, every-tick
≤33 мс и165FPS/every-frame≤6,06 мс не достигнуты. Редкие секундные фризы
и длительный soak этим240-секундным прогоном не закрыты.

## Воспроизведение

Локально `out/optimization-332/`:initial-* отдельно от inline variant,
probe пары, production-after, frozen binaries/sidecars/source-inputs,
full tests,golden,scenario и native evidence. Data/баланс не изменялись.
`production-binary-comparison.json` явно фиксирует различие probe/final.

```powershell
$env:RAYON_NUM_THREADS='4'
./out/optimization-327/after-bench_battle.exe --world --ai-vs-ai --ticks 6000 --physics-workers 4 --workers 8 --tick-csv out/optimization-332/repeat-before.csv
./out/optimization-332/after-bench_battle.exe --world --ai-vs-ai --ticks 6000 --physics-workers 4 --workers 8 --tick-csv out/optimization-332/repeat-after.csv
./scripts/profile-range.ps1 -Seconds 240 -Scenarios world-battle -Output out/optimization-332/repeat-native -SkipBuild -BinaryPath out/optimization-332/after-blast.exe -PhysicsWorkers 4 -NavigationWorkers 8 -RayonWorkers 4 -ComputeWorkers 4 -Maximized
```

Дальнейшая очередь316: persistent provider geometry/membership04a,
активные/изменённые IDs05a и дорогие пики/валка03/02b. Полные обходы
истории и rebuild индекса опор сохраняются. Срез332 устраняет часть
арифметики слияния, а не весь главный расход.
