# ADR-596 — параллельный рельеф локального поиска

Дата: 2026-10-06. Продолжение оптимизации world60 после ADR-595.

## Диагностика

Frozen before — out/fix595/after.exe. Обычный минутный before сохраняет
hash1f697f865e6f8aa6. Дополнительный прогон --profile-workers обнаружил
работу на всех8 navigation workers. Поздняя route_query: суммарные
job wall на worker1265..1741мкс/тик; local_advance552..1061мкс/worker,
span2874мкс, longest3864мкс. Unit22 заканчивает route_query последним
232 раза в последнем окне300тиков. Это хвост отдельных продолжений,
а не доказательство простаивающего пула. Вложенные фазы не суммируются.
Profile-workers добавляет наблюдатель; его время не заменяет тихий A/B.

Широкие terrain-only поиски уже готовили sweep poses независимыми
пакетами. Локальные продолжения с соседями/записью clearance целиком
использовали последовательный рельеф, даже при наличии свободных workers.

## Решение

Широкий локальный поиск готовит только геометрию sweep на неизменном
world/rubble. Первые4 samples дополнительно проверяются на snapshot
blockers чистым предикатом, без записи логических callbacks; известный
отказ оставляет прежний последовательный путь. Геометрический prefix
и оставшиеся независимые batches используют прежнюю подготовку.

Проверки соседей, порядок записи HuntClearance, логический бюджет,
решение пути и принятие результата при пространственном commit остаются
прежними. Каждый worker читает immutable snapshot и свою копию колонок.
Результат возвращается в порядке sample index; кеш — временный, не
snapshot/state. Живые соседи проверяют результат до принятия так же,
как до изменения. Нулевой budget по-прежнему только инвалидирует кеш.

Локальные geometry leaf batches теперь посылают sweep_0..3 события тому
же navigation task observer, что terrain-only. Старый тестовый observer
с последовательным журналом сохраняется; наблюдатели не управляют работой.
Нет изменения collision policy, caps, числа тиков, оружия или материи.

## Протокол и ограничения

Логи/manifests:out/fix596/. Рабочие frozen варианты удерживают одинаковые
унаследованные WIP вне коммита. Публикуемый индекс проверяется отдельно.
Измеряется CPU headless; ни165FPS, ни native/camera эффект из него не
следуют. Первое trial A/B показало лишь небольшой выигрыш, поэтому
нужны повтор и точные проверки состояния/clearance. Это не фикс
физического заполнения пула и P-87 остаётся открытым.

```powershell
$env:RAYON_NUM_THREADS='8'
out/fix595/after.exe --world --ai-vs-ai --ticks 1800 --workers 8 --damage-trace --fire-trace --wave-trace --tick-csv out/fix596/world-before.csv
out/fix596/trial.exe --world --ai-vs-ai --ticks 1800 --workers 8 --damage-trace --fire-trace --wave-trace --tick-csv out/fix596/world-trial.csv
```

Trial сохраняет все12 окон damage_window/ground/storage/laser/admission/
wave_geometry/terrain_melt,143 wave_progress и48 fire_window. Hash тот же.
Motion mean3809→3716мкс,route mean2069→1977мкс,общий tick mean10395→10277мкс;
p95 tick14862→14422мкс,p99 16116→16036мкс,max20112→21283мкс.
Один небольшой выигрыш не считается гарантированным общим ускорением.

Новая release регрессия сравнила старый порядок подготовки и локальные
batches на workers1/4/8, разных snapshot blockers, деревьях/склоне,
после правки мира и загрузки. Полные bytes Motion и точный ordered
clearance journal совпадают. Стенд требует реальные отказанные
clearance и реальные geometry worker batches, а не факт вызова API.


## Повторный тихий A/B

Финальный frozen after.exe SHA256
83AD98A933F423501D15336B96555F9FD79A800EC427BC82F4BDDCBC1A86233A.
Before остаётся out/fix595/after.exe; final after добавляет leaf observer.
Последовательно выполнены world-before-repeat, world-after,
world-after-repeat без сборок/тестов/профиля в фоне. Все сохраняют hash
1f697f865e6f8aa6, все12 окон каждой damage-метки,143 wave_progress,
48 fire_window. Физическая нагрузка совпадает.

| CPU, мкс | Before repeat | Final after | Final after repeat |
|---|---:|---:|---:|
| tick mean | 10290 | 10159 | 10166 |
| tick p95 | 14657 | 14284 | 14218 |
| tick p99 | 15656 | 15942 | 15560 |
| tick max | 21042 | 25206 | 29100 |
| motion mean | 3796 | 3664 | 3651 |
| route mean | 2065 | 1950 | 1943 |
| route p95 | 3420 | 3258 | 3299 |
| route p99 | 3895 | 3868 | 3713 |
| route max | 7902 | 15589 | 8101 |

Mean route сократился примерно на5.6–5.9%, полный тик примерно на1.2–1.3%.
Большой общий выигрыш и устранение максимумов не доказаны. В первом
final after пик tick241:route15.589мс; в повторе этот пик не повторился,
а новый максимум tick271 включает battle_capture11.357мс. Разные пики
не позволяют приписать их одной фазе или объявить причиной OS scheduler
без отдельного stall capture. В следующем разборе сохранить per-frame
phase/cycle/OS evidence, а не выкинуть выбросы из статистики.

Диагностический workers-after1800 выполнен отдельно. Late local_advance
span2874→2150мкс; unit22 job wall2740→1980мкс/тик. Route_query span
3042→2525мкс. Теперь sweep leaves показывают работу unit22 и unit46,
раньше скрытую внутри local_advance; рост учтённой работы sweep не равен
росту физической нагрузки. Времена с observer не заменяют тихий A/B.

Рабочий release lib прошёл654 tests,0failed,4manual ignored,137.03с.
Последующее уточнение только тестового стенда создаёт новый terrain
для каждого случая и требует фактическую dig, а не повторный NoOp.
Окончательный стенд отдельно проверяется в staged export.


Длительный world-long3600 сохранил hash ebbee0fe0becc090 предыдущей
итерации. Он выполнялся вместе со сборкой staged export: времена
этого прогона не используются как evidence ускорения. Это проверка
сохранения результата длинного боя; физические хвосты P-87 остаются.


В staged export без унаследованных WIP финальный local sweep test прошёл
(isolated-local.log). Четыре golden и19 save tests passed
(isolated-golden-save.log); golden не изменены. Все три публикуемых
Rust sources побайтно совпадают с квалифицированным экспортом индекса.


CLI из staged export прошёл:seed42,ticks1000,units5000,threads1,
hash66041a9f60189841 (isolated-cli.log). Рабочая native сборка
сохраняет унаследованные WIP вне коммита; её hash/provenance не следует
выдавать за чистый индекс. Перед сборкой обновлены timestamps всех
tracked Rust/WGSL/RON/Cargo/config inputs и нового test module.


## Native и камера

Fresh native.exe == target/release/blast.exe, SHA256
FEC4B1EB947C5DA70B523F59D237C788D7EF2918D9DFF20B95403E66FA8F01E6.
Manifest проверен:355 refreshed inputs сохранили SHA через сборку и
при завершении квалификации. Состав рабочей копии описан выше.

Последовательный 60с before/after camera tour, RTX5090/Vulkan,
1600×900, physics/navigation8, GPU timestamps, без фоновых сборок:

| Native frame | Before | After |
|---|---:|---:|
| mean FPS | 105.9 | 106.3 |
| p95, мс | 24.140 | 23.779 |
| p99, мс | 27.560 | 27.059 |
| max, мс | 38.283 | 59.044 |
| mean core tick, мс | 11.531 | 11.370 |

Оба runner manifests ValidForComparison/AcceptanceEligible=true,
1800 ticks с realtime ratio≈1, одинаковый hash1f697f865e6f8aa6.
Camera evidence:11 segments,118 разных origins, движение/отдаление
выполнены. stream p99/max3.967/18.475→3.829/15.059мс; parent presentation
уже включает stream, их нельзя складывать. Это один native pair,
общий FPS практически прежний,165FPS не достигнуты.

After max59.044мс на56.495с — interval между началами кадров:
предыдущий кадр21.375мс + gap37.669мс. В следующей строке ноль simulation
ticks; её собственные physics/core таймеры нельзя приписать предыдущему
interval. Причина gap без matching trace/OS evidence не установлена.

Дополнительный trigger25ms + render timeline + CPU threads/OS pressure
(camera-stalls) валиден как диагностика, но AcceptanceEligible=false:
4 windows закончились к tick496,205 frame signals были unarmed.
Поздний симптом невозможно квалифицировать по такому ограниченному ring.
Это проблема выбора протокола: для поздних/непредсказуемых пауз применять
существующий -CorePhases вместо -TriggerMilliseconds. Не выдавать наличие
CSV за доказательство полного контекста максимума.

Полный camera-core60с записал все1800 ticks1..1800 и retained79352 render
spans,0dropped/0incomplete, OS pressure и worker cycles. Hash сохранился;
Validity=true, AcceptanceEligible=false, данные с overhead не заменяют
quiet A/B. Пик37.578мс на56.328с содержит previous frame33.342мс +gap4.237мс.
В tick1689 parent rubble wall10.481мс, motion6.191мс; rubble callback
cycles delta5054220. Longest rubble leaf:columns0.611мс,
floor0.059мс,gather0.027мс. В floor trace jobs видны около0.22–0.41мс,
columns begin9.457мс. Это основание отдельно разбирать dispatch/join,
неучтённые промежутки и планирование ОС, а не объявлять геометрию10мс
активной CPU-работы. Parent/children/cycles относятся к разным потокам.
Точный gap59мс в диагностических повторах не воспроизведён.

Следующие приоритеты: поздние rubble dispatch/join boundaries и frame
handoff/present gaps; затем remaining motion/debris costs. Общие
многосекундные паузы,165FPS и физические хвосты P-87 не закрыты.
