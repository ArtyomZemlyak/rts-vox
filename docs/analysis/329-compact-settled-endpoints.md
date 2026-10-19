# 329. Компактные endpoints таблицы контактов завалов

Дата: 2026-10-19. База: `main 12300a8`. Единственная очередь —
[316](316-optimization-delivery-plan.md). Статус: не принят, runtime прототип удалён.

## Основание и новый срез

Крупнейшая часть позднего debris — построение Settled. В327 late cells
примерно1,3–1,4 мс, весь settled3,2–3,4 мс; той же таблицей пользуется
rubble. Предыдущий [328](328-compact-fragment-landings.md) сократил плотный
массив посадок, но не ускорил targeted translation, и удалён.

Каждая запись `SettledCellStripe.cells` прежде хранила head/tail как
два usize. На x64 полезный tuple key/value занимает32 байта:12 байт
полного signed3D ключа,16 байт endpoints и padding. Обычные node indices
в таблице значительно меньше u32::MAX. Использование двух u32 даёт
20 байт tuple (−37,5% payload), не меняя число клеток или связей.
Это не обещание снижения RSS всей игры на37,5%.

## Точность и пределы

Обычный head/tail содержит исходные индексы arena в двух u32.
Новый u32::MAX sentinel никогда не является обычным endpoint.
Перед первым недопустимым индексом вся таблица stripe переносится
в прежнее полноразрядное представление с теми же endpoints. Перенос
использует sorted_entries, не обход в hash order. Малый test LIMIT
принудительно упражняет переход без много-гигабайтной arena.

Linked nodes, source IDs, wide successor map, порядок контактов,
ключи X/Y/Z, world/hull bounds и запросы прежние. LIMIT — формат индекса,
не лимит числа физических кусков или игровая квота. Wide режим сохраняет
полный usize и после clear переиспользует свою вместимость. Cold/reset
строит обычное представление заново. Дополнительного кэша ответов нет;
геометрия мира и same-tick collision/navigation по-прежнему свежие.

В обычном режиме неизменны hasher и число lookup. Память таблиц меньше,
но widening endpoints и ветвления входят в полный замер. Promotion
потребляет дополнительную память и сортировку; compact capacity после
него удерживается. Это редкий корректный fallback, не квалификация
физического мира с миллиардами nodes. Общий heap/VRAM peak не измерен.

## Адресные проверки

Новый тест сравнивает все head/tail с BTreeMap после каждой вставки,
включает повторные клетки, отрицательные/крайние координаты, LIMIT3,
несколько clear/build поколений и полные usize значения. Проверены
фактические размеры tuples20/32 байта на целевом x64.

Существующий independent cell-list oracle упражняет complete source lists,
негативные границы,1/2/4/8 workers и empty/reused индексы. Wide links с
LIMIT3 одновременно проверяют новое расширение heads. Kernel oracle
сравнивает world/debris/ledger/wrecks с scalar path и restore; rubble
oracle проверяет прежние опоры/колонки. Адресные debug тесты прошли;
полный release-набор и native ещё не квалифицированы.

## Стенд

Baseline frozen327,World AIvsAI6000,physics4/nav8/Rayon4, ordinary CSV,
без detailed rubble spans. Before-a без изменения перенесён из328:
тот же baseline binary/параметры; это не новая соседняя пара. Свежий
reverse baseline необходим для принятия результата. Runtime source
fingerprints и raw — `out/optimization-329/`.


## Результат probe

Mean root16,93089→17,01428 мс, p9944,001→45,891,max105,443→104,832.
Late root19,14666→19,34392,debris5,49717→5,61685,settled2,80117→2,91125,
cells1,16067→1,21742 мс. Все36 non-timing полей6000 шагов совпали,
hash `21bef878463f364c`. Меньший payload не дал меньшей цены builder;
дополнительные widening/dispatch и layout не окупились. Это не доказанная
атрибуция отдельных CPU инструкций. Изменение не принято, старое
представление восстановлено. Повторные full release/native для этой пробы
не запускались. Архив `rejected-compact-endpoints.rs`, fingerprints и
frozen probe сохранены локально. Layout/RSS экономия не выдаётся за
ускорение движка. Уточнённый кандидат — ранний выход при полном skip mask,
без замены таблицы контактов.
