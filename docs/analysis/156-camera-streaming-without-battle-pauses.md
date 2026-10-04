# Камера без многосекундной остановки боя

04.10.2026, ADR-562, исходный `main` `302a784`. WBPERF-009 выполнена частично;
WBPERF-001/027 получают воспроизводимый маршрут камеры и отдельные расходы.
165 FPS, строгая скорость 1× при любых перелётах и причина редких core peaks
остаются открытыми.

## Подтверждённые причины

`Streaming::loading()` считал любую подготовку нового окна загрузкой сцены.
`advance` возвращался до накопления elapsed time, выставляя `skip_delta`.
1280 строк по восемь строк/кадр — 160 кадров без продвижения боя. Новый
запрос камеры отменял незавершённый Job, поэтому при непрерывном движении
окно могло постоянно начинать подготовку заново. Native маршрут подтвердил
6270 blocked frames и sampled no-tick interval **4,202899 с**.

Отдельная ошибка `moving::before`: `occupied.is_empty()` означал отсутствие
инициализации. После выхода всех юнитов за пределы окна корректный пустой
clip заставлял серийно перечислять геометрию всей армии каждый тик.
Инициализация теперь требует отсутствия `visual.window` одновременно с
пустым набором. Корректное пустое окно не запускает этот fallback. Тест
проверяет и переход обратно: новый actor не должен считаться уже загруженным
в пустой GPU shadow volume.

## Изменение

Обычный camera Job не блокирует симуляцию. Барьер новой generation при
замене сцены остаётся отдельным состоянием `invalid`. Job заканчивает
выбранное окно, даже если камера уже запросила другое; следующая подготовка
берёт актуальный запрос. Переход в обзор всей карты отменяет camera Job,
не превращая отмену в сброс сцены.

До восьми строк читаются независимыми jobs в существующем Bevy compute pool.
Чтения используют неизменяемый мир текущего кадра; сборка строк и публикация
упорядочены. Raw material columns пакуются напрямую; косметические изменения
поверхности применяются только к фактическим `surface_work`, без BTree lookup
для каждого неповреждённого вокселя. Нового пула и потоков на кадр нет.

Правки завершённых строк ставят их в bounded repair set. Непрочитанные строки
сразу получают актуальное состояние мира. Окно публикуется после завершения
первичного scan и repairs; живые юниты добавляются обычным occupancy после
публикации. FullWorld clone и изменения авторитетной симуляции не нужны.

GPU patch union включает изменённые строки и физические word addresses,
которые старое окно меняло после сравнения со staging. Это важно для следов
движущихся юнитов: совпадение CPU строки при её первом чтении не гарантирует
совпадения позже загруженного GPU volume. В union попадают только **новые**
патчи завершённого тика, а не весь retained `gas.terrain_dirty` journal.

Union хранится битовой маской по физическим словам. Повторы не накапливают
историю; перед публикацией получаются точные отсортированные ranges.
Для штатного окна 1280×192×1280 маска занимает 4,6875 MiB; восемь row buffers
содержат суммарно 960 KiB packed words. Существующие большие staging/visible
volumes сохраняются; маска не утверждается как уменьшение всей памяти рендера.

## Наблюдаемость

`profile-range.ps1 -CameraTour` / `--profile-camera-tour` запускают маршрут
`bases_pan_zoom_v1`, независимый от тиков и FPS. Восемь остановок по шесть
секунд повторяются: первая база, соседний участок, центр, вторая база,
соседний участок, обзор всей карты и два диагональных перехода. Есть резкие
переходы и плавные pans/zoom. Это native renderer с теми же framing/streaming
системами, без видеозаписи и pixel readback.

`*.camera.csv` хранится в памяти и записывается после замера: время, tick,
segment, requested focus/height, опубликованный origin, progress и blocked.
`observe_ms` и `stream_ms` — отдельные расходы **этого** Update, а не latest
render-thread atomic samples. Обычный режим не включает эти дополнительные
таймеры. Анализатор проверяет declared/runtime mode, наличие артефакта,
число кадров, монотонность, допустимые flags и конечные неотрицательные costs.
Сравнение CameraTour с неподвижной камерой запрещено comparison gate.

`max_no_tick_seconds` — разница между первым и последним camera samples с
одним tick. Это наблюдение остановки на нескольких кадрах, **не** точный
межтиковый интервал: длинный одиночный кадр с новым tick может не попасть в
эту величину. Поэтому рядом всегда приводятся begin-to-begin max/p99 кадров.
Legacy screenshot `--camera-sweep` сохранён отдельным режимом.

Scripted actions явно ждут завершения camera progress перед проверкой
дальнего сектора; ожидание теста больше не означает остановку самого боя.

## Native результаты

Все прогоны последовательные: world-battle 30 на 30, stock tuning,
1600×900 AutoNoVsync, physics=8 / navigation=8 / compute=4, TargetFps=165.
Во время измерений нет concurrent builds/tests/analysis, capture collectors
core/render/tasks или видео. Все результаты сохранены, включая неудачные
промежуточные варианты. FPS ниже — observed cadence, не сертификат 165 FPS.

| CameraTour, 120 с | FPS | Steps | Realtime ratio | Max frame, ms | Max sampled no-tick, s | Blocked frames |
|---|---:|---:|---:|---:|---:|---:|
| before, старый streaming | 106,581 | 2275 | 0,631775 | 55,246 | 4,202899 | 6270 |
| первый неблокирующий вариант | 32,147 | 2432 | 0,675045 | 91,861 | 0,033084 | 0 |
| новые dirty spans вместо всей истории | 57,805 | 3503 | 0,972329 | 79,913 | 0,033171 | 0 |
| bounded bit union, до empty-clip fix | 55,872 | 3419 | 0,949015 | 109,184 | 0,033119 | 0 |
| final, empty clip исправлен | 82,754 | 3599 | 0,999117 | 54,189 | 0,033155 | 0 |
| final repeat | 80,941 | 3599 | 0,999054 | 71,274 | 0,032991 | 0 |

Первая попытка устранила остановку, но создала собственную тяжёлую обработку
patch history: occupancy включая observer в среднем 9,932 ms/frame.
Cursor устранил повторный обход retained history. Битовая маска ограничила
стоимость union, но сама по себе не устранила полную дорогую работу: замер
разделённых scopes показал observe mean 5,8 µs/frame и обнаружил иной расход
при пустом clip. Этот вариант также не проходит FPS/tempo acceptance и
сохранён как промежуточное evidence, а не объявлен успешным FPS gain.

После empty-clip fix оба финальных тура проходят integrity checks, имеют
21 segment, 150 разных published origins и ноль blocked frames.
Observer mean 4,19 / 4,02 µs, p99 70,7 / 62,4 µs; streaming mean 0,448 /
0,447 ms, p99 4,640 / 4,690 ms, max 17,491 / 17,278 ms. Cost включает
small synchronous shifts, scan/repairs и publication, а не только worker reads.

**Оба тура не проходят строгий существующий tempo gate**: 3599 ticks при
120,073 / 120,080 с measured wall. Остался небольшой терминальный debt,
допуски не расширялись. Hash обоих `78753aecc5d80a9f`, полные fuel/stream/
battle lines совпадают. Baseline выполнил значительно меньше тиков и имеет
другое состояние; eligible FPS A/B с ним нет. Более высокий старый FPS не
означает более быстрый бой: симуляция была приостановлена половину кадров.

В repeat max frame 71,274 ms следует за tick2824 с core 57,240 ms. Без
phase trace его физическая причина не установлена. Это не доказывает
устранение прежнего редкого пика core236,65 ms из ADR-561.

### Контроль неподвижной камеры

| 120 с | FPS | p99, ms | Max, ms | Steps | Core/step, ms |
|---|---:|---:|---:|---:|---:|
| before | 85,422 | 30,361 | 50,854 | 3600 | 14,2720 |
| final | 84,860 | 30,403 | 50,655 | 3600 | 14,2107 |

Оба acceptance eligible, comparison eligible. Hash `bcc1f58c226d1721` и
полные gameplay lines совпадают. FPS −0,658%, core/step −0,429%: одной пары
недостаточно для устойчивого небольшого gain/regression, существенного общего
ускорения неподвижного боя не доказано.

### Расстановка, reset и смена сектора

Final native `actions`, 30 с: все восемь steps завершились, damage=263 перед
reset, far origin `[8400,0,8384]`, возврат `[0,0,0]`, generation=4. Max frame
43,576 ms, p99 6,219 ms. 793 ticks / ratio0,8811 включают предусмотренные
остановки и resets этого сценария: не использовать его FPS/ratio как
производительность непрерывного глобального боя.

## Проверки и provenance

Full release render bin: **123 passed, 3 прежних ignored**. Новые regressions
сверяют patch union с dense reference на 64-bit boundaries/повторах;
staged rows, live additions/removals/surface work и поздние old shadows с
direct CPU volume и GPU patch reconstruction при 1/2/4 workers; valid empty
clip и reentry. Последний тест проходит вместе с прежними history/reset,
parallel occupancy, HUD и workshop проверками. Analyzer: **106 passed**.
PowerShell validity/parser suite проходит; Clippy не добавляет предупреждений,
fmt и diff checks чисты. Compile/test ошибки промежуточных правок и исправление
устаревшего test-only assertion сохранены в raw logs.

Все frozen variants в `out/fix562/`, с build manifests. Before
`camera-before.exe` SHA256
`8748195DB6237C59DF0A970293027F25DCD6FE3512EA00D52057F4A17F5B4DA6`;
initial candidate `camera-after.exe`
`8884E81A110719A63F4A4AF885F8451FB973A60DC2DA2FD9FCA70066B63A21BC`;
cursor `camera-cursor.exe`
`B7D9D202036603C583203F895407BACEB54CFCA1E6A04340D252036248F68F2D`;
bit union `camera-bitset.exe`
`D560ACB015812D6D1AA97798805A7DEB860CDD8AFB2AED697C7B74B88F915C04`.
Final `camera-final.exe` и `target/release/blast.exe`:
`8F013BBFE8990D15369747F9428CB9AE2BC6BFC3609EC4639C8D42A553F6BC8F`.

Final manifest проверяет неизменность refreshed blast/profile/camera/streaming/
moving source hashes до и после build; это не clean dependency closure.
Изначальные 13 WIP файлов присутствуют в обеих сторонах и сохранены побайтно;
в смешанных decision docs staged только HEAD + ADR-562 append. Gameplay data
и sim-core не редактировались этим шагом.

Raw directories: `{before,after,cursor,bitset,final}-tour-120`,
`final-tour-repeat-120`, `control-{before,after}-120`, `final-actions-30`.
У каждого CSV/log/manifest, analysis и OS observations; туры дополнительно
содержат camera CSV. Сводка — `out/fix562/final-summary.json`.

## Оставшиеся границы

WBPERF-009 остаётся partial: camera Job больше не теряет секунды боя, но
строгий no-debt критерий не выполнен, стоимость publication/малых shifts
доходит до 17 ms. Чрезвычайно плотные непрерывные edits могут продлевать
repair phase: очередь ограничена, завершение при любом потоке правок не
доказано. Native route не заменяет pixel QA flicker/caves, ручные mouse/minimap
paths и отдельный orbit workload; эти проверки остаются в приёмке.

WBPERF-001/027 получают воспроизводимость и cost separation, но редкие
физические пики, OS scheduling/waits и balanced CPU не закрыты. WBPERF-002 —
по-прежнему синхронный sim/render critical path; worker step с same-frame join
его не устраняет. Нужны owned executor и bounded completed read model,
сохраняющий команды, generation и terrain/hit/death события.
