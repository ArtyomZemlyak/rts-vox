# 330. Ранний выход при полном исключении корпусов

Дата: 2026-10-19. База: `main 12300a8`; очередь —
[316](316-optimization-delivery-plan.md). Статус: не принят, runtime прототип удалён.

## Найденная лишняя работа

Translation составляет примерно1,2–1,5 мс позднего debris. `hull_near`
уже отмечает в apart mask все корпуса, которые данный sweep не встретит.
Substeps и tumble передают маску в `hull_box_hit`. Но даже при исключении
всех корпусов функция вычисляла reach/vertical bounds и обходила весь
roster, отвергая элементы по одному установленному биту.

Если число корпусов не больше числа начальных единичных битов skip,
каждый допустимый source slot уже исключён. Функция сразу отвечает None.
`trailing_ones` возвращает максимум64, поэтому для65+ остаётся прежний
обход: суффикс не скрывается маской. Пустой список включён в то же условие,
сдвига на64 и нового ограничения размера армии нет.

Остальные запросы исполняют прежние geometry/voxel/source-order predicates.
Не меняются сами skip masks, Swept bounds, evict, опоры и коллизии,
частота SIM, logical quotas, сила/радиусы оружия, ledger или save/hash.
Нет нового состояния, таблицы, кэша и аллокации. Это локальное точное
сокращение вычисления ответа, который уже следует из текущего входа.

## Проверки и сравниваемая работа

Новый тест проверяет0/1/2/15/16/63/64/65/96 реальных перекрывающихся
корпусов, несколько масок, box/exact modes и обязательный контакт с первым
не исключённым source slot, включая65-й. Existing independent query
oracle покрывает turns/voxel holes/skip masks. Полный kernel snapshot
oracle и zero-room eviction за исходный sweep прошли в debug.

Перед этой пробой отвергнуты [328](328-compact-fragment-landings.md)
и [329](329-compact-settled-endpoints.md): первый не ускорил targeted
translation, второй замедлил cells. Их runtime удалён, исходники и
замеры сохранены отдельно. Данные этих вариантов не приписываются330.

World AIvsAI6000,physics4/nav8/Rayon4, ordinary CSV, без detailed rubble
spans. Baseline frozen327. Before-a перенесён без изменения из328
(тот же baseline binary/параметры); это не новая соседняя пара. Для
приёмки нужен свежий reverse контроль. Runtime inputs и raw —
`out/optimization-330/`. Полный release/native gate ещё не завершён.


## Результат probe

Mean root16,93089→17,06698 мс,p9944,001→43,944,max105,443→103,232.
Late root19,14666→19,31170,debris5,49717→5,45710,translation1,27808→
1,27458 мс. Все36 non-timing полей6000 шагов и hash `21bef878463f364c`
совпали. Экономия targeted translation около0,3% в одном probe при
более дорогом root; причинное ускорение полного боя не подтверждено.
Вариант удалён. Полная release/native квалификация не проводилась.
Архив `rejected-full-skip.rs`, binary/fingerprints и результаты сохранены.
Дальнейший кандидат — повторная геометрия составляющих клеток внутри
одного rigid physical step; это не возвращение отвергнутого persistent
local-AABB sidecar322.
