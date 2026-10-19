# 331. Геометрия составляющих клеток внутри одного движения обломка

Дата:2026-10-19. База:`main 12300a8`; очередь —
[316](316-optimization-delivery-plan.md). Статус: не принят, runtime прототип удалён.

## Повторная работа

За один rigid substep sequence положение меняется, но shape/side/mm/basis
не меняются. `fragment_contact` и `fragment_motion_contact` повторно
вычисляют радиус AABB каждого составляющего вокселя, центр масс и
преобразованный offset по тому же basis. World contact/support predicates
по-прежнему должны читать текущую геометрию; сохранять их ответы нельзя.

Первый кандидат322 кэшировал local bounds между фазами и не ускорил SIM.
Этот вариант иной: дорогая constituent geometry используется внутри
одной операции, без сравнения/версий всей холодной истории. Предыдущие
[328](328-compact-fragment-landings.md), [329](329-compact-settled-endpoints.md),
[330](330-complete-hull-skip-mask.md) удалены после недостаточных результатов.

## Ленивое вычисление и актуальность

При первом реальном broad contact сохраняются radius/centre/scale и offset
первого составляющего вокселя, с тем же порядком Fx-операций. Дальнейшие
offsets вычисляются только при достижении соответствующей проверки,
в исходном порядке битов shape. Первый offset inline; дополнительный Vec
выделяется только при необходимости второго. Не вычисляются заранее все64
клетки, когда ответ уже дал первый контакт. Box/chunk и single-voxel paths
не создают constituent cache.

Объект живёт только внутри substeps. Evict выполняется до его создания;
последующий tumble проверяет новый basis прежним raw predicate. Нет
межтикового кэша, сохранённых ответов мира или пропуска support queries.
Порядок axis/substeps, displacement, bounce/friction, hull/settled queries,
физическая вместимость и ledger прежние. Shapes вне допустимого bit-domain
сохраняют прежний обход только box_cells(side) битов.

Сохраняются точные Q32.32 выражения: нет перестановки умножений/сложений
или усреднения формы. Snapshot/hash и оружие/радиусы не меняются.
Оригинальный raw contact/motion путь остаётся самостоятельным oracle
scalar kernel и путём dead hulls. Память живёт у выполняемых операций,
не сохраняется на всю историю. Снижение RSS/VRAM не измерено.

## Проверки перед замерами

Direct differential test сравнивает actual hit и separation для sparse/full
2³/4³ shapes,50/100/150/400 мм, поворотов, разных высот и каждого axis
в обе стороны. Положение меняется при одном cache, cached prefix растёт
только при необходимости. Требуются и hits, и misses. Single-cell path
не создаёт cache. Kernel snapshot oracle проверил мир/debris/ledger/wrecks,
движущиеся корпуса, резервы/claim и restore; zero-room eviction сохранён.

Это адресные debug проверки, не полный release/native gate. Baseline
frozen327, ordinary World AIvsAI6000,physics4/nav8/Rayon4. Before-a взят
без изменений из328: тот же baseline binary и параметры. Нужен свежий
reverse контроль. Исходные fingerprints/raw — `out/optimization-331/`.


## Результат probe

Mean root16,93089→17,07211 мс,p9944,001→44,443,max105,443→106,310.
Late root19,14666→19,36494,debris5,49717→5,50700,translation1,27808→
1,29958 мс. Все36 non-timing полей6000 шагов и hash `21bef878463f364c`
совпали. Геометрический кэш не окупил хранение/ветвления/аллокации;
точная CPU-атрибуция отдельных расходов не выполнялась. Вариант удалён,
полная release/native квалификация не запускалась. Архив
`rejected-rigid-cells.rs` и binary/source fingerprints сохранены.
Следующий проверяемый расход — три runtime Euclidean divisions для
ключа каждой resting pile, при stock pile_cell16.
