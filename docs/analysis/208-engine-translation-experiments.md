# Проверка хранения и сбора результатов движения обломков

Дата: 07.10.2026. Продолжение принятого [207](207-engine-compact-wake-actions.md), часть EP-07 [общего плана](200-engine-performance-plan.md).

## Гипотеза перед изменением

В принятом контроле 207 на tick 3600 остаётся 39 495 fragments, из них 3649 flying. Translation вызывает `advance_fragment` для каждого тела; resting возвращает false сразу. Однако collector записывает `Option<Wreck>` для каждого source slot, затем создаёт второй список посредством flatten. В окне 3301–3600 контроль 207 измеряет translation около 0,535 мс/tick, включая dispatch/join и сбор результатов.

Первая гипотеза: сохранить все вызовы физики и исходные skip indices, но собирать только фактические приземления по упорядоченным batches. Целевая метрика — translation и debris в полном AI battle, gate — отсутствие роста полного tick. Oracle — прежний dense collector, все изменения тел, source order wrecks, полное физическое состояние, ledger, reservations и поздний replay. Сценарии, квоты и физические predicates не меняются.

Все три варианта отклонены. Ни один не подтвердил улучшение полного AI battle tick; рабочий код возвращён к принятому 207. Это проверка гипотез, а не новое ускорение игры.

## Отклонённый первый вариант

Первый prototype вызывал translation closure для каждого элемента исходного массива, собирая только `Some(Wreck)`. Indexed batches из 256 тел соединялись по номеру. Все 848 nav/sim-core tests прошли. Четыре полных AI runs сохранили hash, все 25 per-tick workload fields и physical damage windows, но средние двух контролей/двух prototypes дали translation **+14,88%** (0,529→0,608 мс), debris **+3,07%** (2,523→2,601 мс), полный tick **+1,06%** (13,193→13,333 мс). Он отклонён и заменён; отдельный replay для этого отклонённого пути не выполнялся.

Цена фиксированных batches, их dispatch и соединения на реальной неоднородной нагрузке оказалась выше выигрыша от пустых slots. Причина регрессии не изолирована отдельным experiment, поэтому нельзя приписывать её только imbalance. Frozen prototype SHA256 `f61785a6c5e78ab3f47e1cc8e206728138841d3e45a44688c21e64a1ae7cbbe0`, полный source/manifest/tests/четыре log+CSV/comparison — `out/engine-fixes-20261007/landing-actions/`.

## Отклонённый второй вариант

`advance_fragment` первым условием возвращает false для resting, до чтения геометрии или изменения тела. После ordered wake известен актуальный resting каждого фрагмента. Вторая гипотеза: собирать только движущиеся входы с исходным slot и сохранить прежний indexed `par_iter_mut/map_init` для самой физики. Это сокращает размер списка задач и Option результатов, сохраняя прежнее разбиение Rayon на выбранном наборе.

`fragment_landings` последовательно выбирает `(source_index, &mut Fragment)` при `!resting`, в исходном порядке и после всех wake reservations. Frozen Settled остаётся полным, включая спящие тела и gaps; skip передаётся исходный, а не компактный номер. Контакты, substeps, translation mutations и merge/ledger commit прежние. Wrecks соединяются indexed collect в source order, held IDs не сортируются. При менее 1024 исходных тел выбранные тела обрабатываются последовательно; порог параллельного пути прежний. Observer сохраняет translation begin/end, количество задач может отличаться.

Новый список временный, не входит в save/hash и не меняет ownership, claims, thermal reserve, вместимость или сценарии. Полный scan resting flags ещё сохраняется. В плотном flying случае дополнительный список ссылок стоит памяти и времени; выигрыш нельзя предполагать без полного battle gate. Это производный active input одного прохода, не постоянные active/sleep sets и не миграция хранения.

Второй вариант тоже отклонён: четыре полных runs сохраняют physics/workload, но средние translation **−1,46%** (0,532→0,524 мс), debris **+0,31%** (2,500→2,507 мс), полный tick **+1,24%** (13,155→13,318 мс). Ускорение target phase слишком мало и не подтверждает улучшение полного шага. Его source, 361 inputs, frozen binary SHA256 `dba2d6c42c92156a0ea4c4ff4d83544a0408824190e6c54791ae1fa4efad5dbc`, 848 прошедших тестов и четыре runs сохранены в `out/engine-fixes-20261007/active-translation/`. Отдельный replay для отклонённого варианта не выполнялся; active input selector в рабочем коде не оставлен.

## Отклонённый третий вариант: reuse исходного dense буфера

Третья гипотеза сохраняет исходный полный список translation jobs. Вместо нового dense Option массива на каждом шаге `Bursts` хранит производный `landing_scratch`. Буфер приводится к текущему числу тел и **каждый** slot перезаписывается результатом прежнего `advance_fragment`, включая None для resting. Индексы, порядок и разбиение Rayon остаются source-indexed; новый набор active references и фиксированные batches отсутствуют.

Параллельный проход соединяет непересекающиеся mutable fragments и result slots посредством indexed zip. После join последовательный filter_map копирует только фактические Wreck в прежнем source order. В итоговой pile/ledger фазе ничего не меняется. Старые значения scratch никогда не выбирают физическую работу и не читают ledger; буфер используется только после полной перезаписи. Все physical predicates и вызовы сохраняются.

Буфер сохраняет allocation high-water вместо allocation/free каждый тик. Это дополнительная удерживаемая временная память, пропорциональная peak roster, а не сокращение всех allocations или общий memory budget. Он не входит в save/hash, создаётся пустым после restore, не добавляет поля в Fragment и не меняет сценарии, claims, thermal reserve или квоты. Нет новых handles, active/sleep migration и повторно используемой геометрии.

## Проверки второго варианта

Новый differential test сопоставляет collector с прежними dense Option slots и flatten для 0/1/1023/1024/1100/39 495 тел, 1/4/8 workers: все sleeping; смешанный набор с редкими/плотными приземлениями; все flying с пустыми/плотными результатами. IDs идут против source order, held IDs повторяются. Callback возвращает None для resting как физический translator, а flying меняет даже без приземления; сравниваются все изменения и порядок wrecks. Счётчик требует len вызовов dense и ровно flying вызовов выбранного пути; нет пропуска ни одного активного тела.

Полный physical differential fixture использует компактные wake/landing и indexed hulls против dense collectors и scalar hulls: 1100 fragments, 80 корпусов, перекрывающиеся moving hulls с противоположными push, terrain claim, thermal reserve, двадцать шагов и restore одного пути. Snapshots включают тела/мир, полный ledger/free-list и wrecks. Дополнительные существующие тесты требуют фактическое приземление, локальное слияние, сохранение материи и реклейм.

## Проверки третьего варианта

Oracle сравнивает reuse с первоначальным dense collect/flatten при 0/1/1023/1024/1100/39 495 телах и 1/4/8 workers. Все sleeping, смешанный roster с редкими/плотными landings и все flying с пустыми/плотными результатами. IDs обратные, held IDs повторяются; состояние каждого тела и wreck order равны. Ровно len callbacks на каждом пути. Один scratch переживает смену dense→empty→sparse workloads, shrink roster вдвое и повторный рост: прежние landings не просачиваются в новую фазу.

Полный physical differential fixture третьего варианта сравнивал reuse/compact wake/indexed hulls против original dense collectors/scalar hulls. Он сохранял claims/reserves/ledger и restore одного пути. Для внедрения дополнительно требовался бы late replay оптимизированного пути с пустым scratch после загрузки; вариант отклонён по performance gate до этой проверки.

## Результат третьего варианта и решение

Четыре последовательных AI vs AI runs по 3600 ticks, physics/nav workers=8/8, одинаковые damage trace/CSV. Две контрольные сборки 207 против двух reuse runs дают translation **−1,09%** (0,530→0,524 мс), debris **−0,51%** (2,519→2,507 мс), полный tick **+0,12%** (13,193→13,209 мс). Четыре прогона не дают доверительного интервала; столь малую разницу нельзя считать доказанной причинной регрессией, но доказанного ускорения всего шага тоже нет. Удержание нового буфера памяти при таком результате не оправдано.

Все 848 nav/sim-core tests прошли, renderer blast check и форматирование источников прошли. Все 25 нетайминговых CSV fields на каждом tick, physical damage windows, geometry/ground counters, итоговые shots/damage/alive/bases и hash `5dd0c782d2b2f557` совпадают. Frozen prototype SHA256 `7b36e27d72fe309d0f3676dac85b7aee4d7bd9b3ba04497111abdc916075e3a1`; source/361 inputs/tests/checks/четыре log+CSV/comparison в `out/engine-fixes-20261007/translation-scratch/`. Отдельный late replay для этого отклонённого варианта не выполнялся.

Всего проверены **12 полных боёв**, по четыре на вариант. Сборки/тесты не шли одновременно с замерами. Все три совпали по реальной боевой работе, но gate полного времени не пройден. Первый компактный output и второй active input полностью убраны, поле `landing_scratch` третьего тоже убрано. Канонические arrays, Fragment layout, исходный translation collector, wake из 207 и остальные принятые изменения сохранены. Нет нового engine API, сохраняемого состояния, квоты или сценарного обхода.

Во время начала повторных runs выполнялся короткий предварительный разбор двух завершённых CSV. Этот фон и обычный шум машины не изолированы; особенно разницу +0,12% третьего варианта нельзя приписывать исключительно коду. Решение означает отсутствие достаточного подтверждения полного выигрыша, а не доказательство точной причинной цены каждого прототипа. Новый retained scratch при столь малом target effect не принят без более сильного полного результата.

Это не доказательство, что allocation или обход сна всегда несущественны. Профиль translation содержит вычисления, dispatch/join и обработку результатов; отдельная цена allocation здесь не изолирована. Локальное уменьшение результатов не гарантирует снижения общего CPU/cache/allocator overhead. Не повторять эти wrappers или новый scratch без более сильной гипотезы и полного gate.

## Проверка отката и следующий шаг

Проверка восстановленного дерева, executable и replay записывается отдельно в `out/engine-fixes-20261007/translation-restored/qualification.json`. Контроль — принятый 207, SHA256 `bb671d9ec7f4f25f53b16fb0bb62974527a0c9d314c14ec5b83d25eeca0a9e0e` и 361 input. Проверки prototypes не выдаются за проверки другого дерева.

Первый restore attempt выявил stale Cargo cache: Copy-Item сохранил LastWriteTime старого backup, поэтому Cargo пропустил compilation и main executable остался **побайтово третьим prototype**. Source passport совпадал с 207, gameplay hash/replay тоже, но executable passport не прошёл; эти tests/replay не являются приёмкой восстановленного дерева. Их logs сохранены в `translation-restored/stale-cargo-cache/`. Даты двух возвращённых исходников обновлены без изменения bytes, после чего выполняется настоящая пересборка и новые tests/replay. План теперь явно требует этот порядок при restore, плюс проверку фактического binary и input hashes. Считать executable принятым только по совпадающему gameplay hash нельзя.

После обновления дат действительно пересобран sim-core: **847 passed, 0 failed, 6 ignored** (111 nav и 736 sim-core). `render --bin blast`, форматирование восстановленных/замороженных источников и `git diff --check` прошли. Отдельный replay свежего binary 3300→3600 прошёл: snapshot 134 186 763 bytes, layout 59 377 bytes, hash `5dd0c782d2b2f557`. Четыре прежних architecture failures из 201 не перезапускались; весь workspace зелёным не объявляется.

Все 361 inputs восстановленного дерева совпали с принятым 207. SHA пересобранного main/frozen executable `980088512ddeed97810769c03c6ad321f247d3dac5863d66977f72891fdb8e76` записан отдельно от старого контроля. Машинный код `.text` побайтово равен контролю: SHA256 `0fa5d8e6482c3abfb8830682603f278b13c1a952ef20fb2b1ec28eb4e553c2ce`; также совпали `.data`, `.pdata`, `.reloc`. Полные executable SHA не одинаковы; различия PE сохранены в `pe-comparison.json`, они не скрыты заменой файла контрольным binary. Проверены и все 361 inputs каждого frozen prototype по сохранённым source copies; каждому записан `rejection.json`. Итоговый паспорт отката — `translation-restored/qualification.json`.

EP-07 остаётся открытым. Общий active/sleep index должен сокращать **полную** работу истории и сохранять source slots, frozen contact snapshot, due schedule и ordered wake admission. Временная фильтрация одного входа и перестройка коллектора не закрывают эту задачу. EP-02a/02b/03 и EP-04b/c сохраняют приоритет: общий командный протокол, согласованное чтение и отдельный владелец симуляции, прогресс физических очередей. Не задерживать эти задачи ради следующего варианта буфера результата. Native FPS и большой полноценный roster этим исследованием не квалифицированы.
