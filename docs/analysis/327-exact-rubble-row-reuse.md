# 327. Точное повторное использование строк карты завалов

Начало: 2026-10-18; итоговая проверка: 2026-10-19. База: `main 2e4ca6a`. Активная очередь —
[316](316-optimization-delivery-plan.md), OPT316-04b.

Статус: принят узкий точный срез SIM, ADR-749. Headless/native бюджеты
полностью не достигнуты; отдельные ограничения приведены ниже.

## Гипотеза и контракт

Предыдущие варианты [324](324-hull-support-candidates.md),
[325](325-settled-column-heads.md), [326](326-hull-roof-levels.md)
не ускорили полный тик и удалены из production. Уменьшение числа
кандидатов или времени одной внутренней операции недостаточно.

Этот вариант сохраняет результат целой X-строки rubble columns только
при точном равенстве полного свежего списка `RubbleRowSpan`: интервалы Z,
top и маска исключающих корпусов, в прежнем порядке. Ключ не является
хешем: коллизии не допускаются. Координата X и границы строк также входят
в сравнение. Удаление прежнего максимума меняет вход и пересчитывает строку.

Классификация world-floor/stacked, текущие bounds и проверки поддержки
продолжают выполняться каждый тик. Ответы `carries` и `supports`, voxel
callback, геометрия мира и состояние материала не кэшируются. Поэтому
изменённая в этом тике дыра сразу меняет состав свежих вкладов строки.

Позднее построение максимумов читает только `Hull::covers`: pos.x/z,
half.x/z, heading. Эти поля сравниваются по source index. При изменении
влияющего корпуса строка пересчитывается даже при равных span descriptors.
Изменение solid/крыши/pos.y обрабатывает свежая классификация выше.

Сохраняются 64 строки на пакет, прежние pool/parallel gates, source order,
полный диапазон top0..65535, старый sparse/wide fallback. Legacy emitter
инвалидирует общий scratch до использования своих allocation swaps.
Переходы serial/parallel и изменение размеров не используют чужую X-строку.

Кэш — дополнительная память: прошлые descriptors, offsets и результаты;
снижение RSS/VRAM не заявляется. Это не persistent support index04a и
не разрешение читать устаревшую физику. Частота SIM, оружие/радиусы,
logical quotas, материя и snapshot/hash не меняются.

## Проверки

Независимый поклеточный BTreeMap oracle дополнен тёплыми повторными вызовами,
изменением heading/позы корпуса, высоты и порядка фрагментов, перестановкой
корпусов и dense legacy fallback между вызовами. Проверяются1/2/4/8 workers,
clipping отрицательных координат и top0/65535. Первоначальная ошибка обмена
старых выходных буферов с legacy emitter воспроизведена и исправлена.

Локальные артефакты — `out/optimization-327/`. Fixed-work baseline:
frozen323 `after-bench_battle.exe`, World AIvsAI6000,physics4/nav8/Rayon4.
Before-a перенесён без изменений из reverse326: это тот же baseline binary.
Результат нового кандидата пока не квалифицирован.

## Подробный профиль6000

| Прогон | Mean root, мс | P99 | Max | Late root | Late rubble | Late columns | Late merge |
|---|---:|---:|---:|---:|---:|---:|---:|
|before-a, reverse326|16,64829|42,515|104,975|18,79449|5,40884|2,00428|0,83948|
|after-a|16,47141|42,930|103,083|18,61770|5,12264|1,67952|0,52608|
|after-b|16,55013|44,386|102,949|18,71529|5,13675|1,64769|0,49826|
|before-b, свежий reverse|16,55252|43,030|104,661|18,66688|5,35916|1,98310|0,83331|

Late —2401..6000. Columns дешевле16–17%, merge37–40%, вся rubble4–5%.
Root mean первой пары−1,06%, reverse пары практически без выигрыша;
поздний root во второй паре даже чуть выше. Debris/motion там также
дороже. Поэтому по этим данным ускорение всего тика устойчиво не доказано.
P99 root выше в обоих after; every-tick≤33 мс не пройден.
Parent/leaf spans перекрываются и не складываются как независимые расходы.

На всех6000 шагах совпали36 non-timing fields, итоговый hash
`21bef878463f364c`. Это одинаковая физическая работа; результат не получен
удалением обломков, уменьшением радиуса или переносом разрушения.

Probe source сохранён отдельно: `probe-row-reuse.rs`,
`probe-source-inputs.json`, `probe-bench_battle.exe`. После probe менялись
только тесты и форматирование; итоговый production build фиксируется отдельно.

## Память и пределы

К предыдущему scratch добавляются16 байт на span-row descriptor,
12 байт на retained output column и две usize-границы на X-строку
(логические payload на x64; Vec capacity может быть больше). При обмене
буферов существуют прежний и новый результаты. Отдельные выделения keys
и output offsets входят в измеренное время, не исключены из root.
Нет заявленного уменьшения heap/RSS и нет измерения аллокаторного peak.
Кэш ограничен размером уже допускаемого полного span/column входа;
не создаёт дополнительных физических сущностей или игровых очередей.

Холодный и полностью меняющийся вход платят за сравнение/копирование.
Small/sparse/wide fallback прежний. Полный индекс membership04a,
событийные зависимости опор и active IDs debris05a остаются открыты.


## Probe ordinary6000 без подробных таймеров

| Метрика | Before | After |
|---|---:|---:|
|Mean root, мс|16,55948|16,44900|
|P99 root|42,697|42,770|
|Max root|104,511|102,796|
|Late root mean|18,70470|18,54903|
|Late rubble mean|5,30603|5,07061|
|Late debris mean|5,24316|5,26440|

В этой паре mean root−0,67%, late root−0,83%, late rubble−4,44%.
Это не ускорение всего движка на16–17%: такая величина относится только
к columns внутри rubble. P99 root практически прежний и чуть выше.
36 игровых полей6000 шагов и hash совпали. Ordinary probe отделён от
подробного профиля; окончательная сборка с дополнительными тестами
квалифицируется отдельно. Golden и native результаты итоговой сборки приведены ниже.


## Полная проверка итогового исходника

Release SIM:1010 passed,0 failed,16 ignored,216,56 с; сборка тестов4м12с.
Cached source-check executables: architecture12/12,docs3/3. Проверка342
Markdown файлов и1271 локальной ссылки: новых ошибок0;35 общих текстов
и73 замены исторических дублей сохранены. Тесты не заменяют нагрузочный
контроль:5000 реальных юнитов,30-минутный soak и редкие секундные фризы
отдельно этой серией не квалифицированы.


## Итоговая production сборка, ordinary6000

Locked/offline сборка bench_battle/bench_rubble_snapshot/sim-cli/blast:
2м45с. Все497 исходных входов проверены перед freeze; production runtime
отличается от probe только тестами/форматированием в исходном файле.
Повторно замерены именно frozen итоговые бинарники, а не подменены
результатами probe. Source/build sidecars фиксируют WIP fingerprints;
это не clean-build/toolchain attestation.

| Метрика, мс | Before | After |
|---|---:|---:|
|Mean root|17,26586|17,06208|
|P99 root|45,816|44,692|
|Max root|105,860|108,409|
|Late root mean|19,59237|19,29706|
|Late rubble mean|5,59782|5,31796|
|Late debris mean|5,55389|5,53084|

В этой паре root mean−1,18%, late root−1,51%, late rubble−5,00%.
P99 ниже, однако max выше: устранение пиков не доказано. Абсолютные
времена этой серии выше предыдущего probe, поэтому сравниваются пары
внутри каждой серии; смешивать их как один ряд улучшений нельзя.
Все36 non-timing полей6000 шагов совпали, hash `21bef878463f364c`.

Golden600:1/4/8 physics workers — `635200a9c3fc1530` во всех случаях.
Итоговый `--support-levels` проверен на snapshot6000: hash сохранён,
распределение совпадает с326. Это диагностическое чтение, не warm-cache
benchmark и не дополнительный алгоритм физики.

## Дополнительные сценарии итоговой сборки

| ID | Тиков | Mean root before→after, мс | Mean rubble before→after | Hash |
|---|---:|---:|---:|---|
|4|900|1.86599→1.78691|0.49043→0.41126|`39ed0e187213fa88`|
|5|900|2.40914→2.30332|0.65233→0.55379|`1fd7abba7eec5963`|
|12|4000|3.52769→3.34965|1.25661→1.10297|`354e0b04fed1bec1`|
|15|4000|1.96342→1.83738|0.78243→0.67253|`4eb06a9e3e77a598`|
|16|4000|2.27466→2.13667|1.12682→0.99036|`3edaedcda5bda080`|

Root дешевле примерно4–6%; на каждом шаге совпадают36 non-timing полей,
итоговые хеши тоже равны. Это пять одинаковых deployment→battle runs,
не прохождение задач MissionRun, не все26 сценариев и не5000 юнитов.

## Ordinary native240, итоговый полигон

Stock,physics4/nav8/Rayon4/compute4, развёрнутое окно3840×1949,focus=true,
scale1,AutoNoVsync,Windowed,view1/zone6,overlays[true,true],drawer0,
камера неподвижна. Подробного CPU/GPU/OS trace нет; соседние тяжёлые
процессы этой проверки не запускались.

Анализатор `--require-valid` принят: evidence_valid=true,
fps_eligible=false,realtime=false. За240 секунд6871 шаг,
ratio0,9539704; mean38,5 FPS,p95 кадра42,474,p9957,881,max124,580 мс.
Core20,47759 мс на фактический шаг. Конечный debt≈29 тиков,
списанное pacing-время300,428 tick-equivalents, без удаления SIM-шагов.
Конец:76856 fragment records,6479516 felling_pending,
hash `846a1e7a9149fabe` на6871.

Предыдущий native323 имел39,7 FPS и6926 шагов за тот же wall interval.
Это **не matched FPS A/B**: дата/состояние/число шагов различаются;
прирост FPS не заявляется и наблюдённое значение здесь ниже. Синхронный
Main Update, рост истории/очередей,30Гц без списания времени,165FPS и
редкие секундные фризы остаются открытыми. CPU fixed-work gain доказан
отдельными равными шагами выше. Ограничения native не переименованы в успех.

## Воспроизведение и дальнейшая работа

Локально `out/optimization-327/`: frozen binaries/sidecars/source-inputs,
probe и итоговые ordinary CSV/log отдельно, scenarios, golden, native240,
тесты и анализаторы. Rejected324–326 имеют собственные source archives
и fingerprints; их результаты не приписываются принятому алгоритму.

```powershell
$env:RAYON_NUM_THREADS='4'
./out/optimization-323/after-bench_battle.exe --world --ai-vs-ai --ticks 6000 --physics-workers 4 --workers 8 --tick-csv out/optimization-327/repeat-before.csv
./out/optimization-327/after-bench_battle.exe --world --ai-vs-ai --ticks 6000 --physics-workers 4 --workers 8 --tick-csv out/optimization-327/repeat-after.csv
./scripts/profile-range.ps1 -Seconds 240 -Scenarios world-battle -Output out/optimization-327/repeat-native -SkipBuild -BinaryPath out/optimization-327/after-blast.exe -PhysicsWorkers 4 -NavigationWorkers 8 -RayonWorkers 4 -ComputeWorkers 4 -Maximized
```

Приоритет единой очереди316 сохраняется: свежий persistent membership04a,
оставшиеся зависимости04b и active/dirty debris05a; диагностика очереди
валки03 и пиков02b параллельны по планированию, не по benchmark запуску.
Rubble/debris остаются сопоставимыми расходами. Результат этого среза —
устранение части повторных максимумов, а не полная оптимизация опор.


### Свежий reverse native-контроль

После after отдельно выполнен baseline323, те же240 секунд,3840×1949,
focus/present/camera/workers/stock совпадают. Evidence valid=true,
FPS eligible=false, realtime=false. Before:6866 шагов,ratio0,9531446,
38,6 FPS,p9542,444,p9958,241,max122,721 мс,core20,61211 мс/шаг.
After:6871 шаг,38,5 FPS,p9957,881,max124,580. Прирост FPS не доказан;
величины mean близки. Это свежий контроль, а не сравнение только с323
из предыдущей сессии. Пики в after не устранены.

Из raw CSV выделено ровно по одному фактическому шагу каждого tick1..6000.
Для всех6000 совпали13 опубликованных игровых полей, включая fragments,
felling_pending, waves, pending edits и moving/searching units. На этих
общих шагах mean nested core19,79896→19,66895 мс (−0,66%), late2401..6000
22,51929→22,33213 (−0,83%). P99 core49,214→49,417 мс: хвост не улучшен.
Это сравнение CPU работы по одинаковым tick IDs под графической нагрузкой;
не matched FPS A/B и не доказательство отсутствия всех редких регрессий.
Конечные tick/hash различаются, их равенство не требуется и не заявляется.
Артефакты: `native-before240/`, `native-common-ticks.json`.
