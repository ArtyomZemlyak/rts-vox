# 436 — Глобальная архитектура: Active World + Resting World

1 ноября 2026. Стратегическое дополнение к [427](427-engine-performance-review.md),
[434](434-retained-state-research.md) и [счётчикам 435](435-retained-work-counters.md).
Это направление архитектуры, не реализованный ADR и не обещанный FPS.

## 1. Пересмотренный главный вывод

Недостаточно оптимизировать `Settled`, join, wake и rubble по отдельности. Пока
каждая подсистема хранит свою копию полной истории и решает инвалидацию сама,
движок будет повторять одну работу и накапливать сложные кэши.

Глобальное решение:

> **Логический обломок не обязан быть вечно обслуживаемым физическим телом.**
> Активный мир содержит движущееся и изменяющееся. Спящий мир хранит точные
> записи, геометрию и экономику, но обновляется только по событиям и запросам.

При пробуждении должны восстановиться те же `FragmentId`, `held`, Fx pose,
geometry, material/paint, mass/count/chunk и канонический порядок. Это не
запекание в terrain, не удаление debris и не отключение reclaim.

Целевая стоимость тика:

`O(active + due + mutations + affected cells + actual query work)`.

Логических записей может быть миллион; горячих физических тел — только несколько
тысяч. Точная история по-прежнему занимает память, но её объём не должен
определять цену каждого тика.

## 2. Почему прежних направлений недостаточно

Счётчики 435 показали:

- selected `Settled` — около 94% истории в позднем бою;
- 424 317 cell links создаются за тик в позднем окне;
- join memo недействительно 1576/1600 тиков;
- 136 036 grouped inputs обслуживаются при 39,55 transfers.

Кроме того, текущий `fragment_join::Geometry` ограничен `u64` mask, стороной 4
и менее чем 64 занятыми клетками (`fragment_join.rs`, 5–25, 38–107). Он сохраняет
геометрию точно, но не является неограниченным представлением большой кучи.
Даже идеальный merge-aware memo не превращает этот формат в полное решение роста
числа объектов.

Поэтому:

- persistent `Settled` устраняет rebuild, но не всю историю в других фазах;
- merge-aware memo устраняет лишнюю группировку, но не ограничивает canonical N;
- compound join меняет физику, если спящие кучи начинают двигаться как одно тело;
- простое удаление/запекание обломков нарушает экономику и запрещено.

## 3. Целевая структура

### 3.1 Active World

Содержит только:

- летящие и пробуждённые фрагменты;
- фрагменты с изменившейся позой/геометрией/материей в этом тике;
- тела, волны, юниты и другие действительно активные сущности;
- ordered mutation journal текущего тика.

Интеграция, contact order и Fx arithmetic остаются прежними. Здесь нет снижения
частоты физики или изменения реакции разрушения.

### 3.2 Resting World — единый владелец спящих данных

Хранит канонические холодные записи и производные представления:

1. **Cold pages.** Упорядоченные точные записи спящих фрагментов. Страница не
   является terrain и не меняет владельца/экономику.
2. **ID map.** `FragmentId → page/slot/active slot`; отдельный sequence/order
   сохраняет каноническую последовательность независимо от compaction.
3. **Spatial index.** Frozen bounds/cell membership с exact query gating.
4. **Due schedule.** Список/колёсо сроков проверок опор по stable IDs, включая
   retry после `damage_waiting`; нет полного сканирования ради поиска due IDs.
5. **Join membership.** Key, geometry version и ordered heads по stable IDs.
6. **Dirty pages.** X/Z rubble, render/publication и nav затрагиваются только в
   изменённых областях.

Холодная страница может хранить полную запись, а hot index только необходимые
поля. При wake запись извлекается в Active World; соседи не обязаны становиться
активными. Snapshot сериализует каноническое состояние, а не случайный layout
страниц и не производные кэши.

### 3.3 Один ordered Mutation Journal

Все изменения записываются один раз в каноническом порядке:

`ID, old/new state, old/new pose, old/new key, geometry version, affected cells`.

Журнал образуют admission, wake, translate, merge, compaction, damage и restore.
Его читают `Settled`, join, rubble, render/nav и публикация. Не должно быть пяти
независимых способов догадаться, что один фрагмент изменился.

Журнал не является новым gameplay state: его можно перестроить из canonical state,
он не входит в hash. При сомнительной/пропущенной инвалидации выполняется полный
rebuild соответствующего производного представления.

### 3.4 Запросы вместо полных проходов

- Flying/due создают прежние query ranges.
- Persistent spatial index возвращает кандидатов только после точного `want` gate.
- Cell lists сортируются каноническим sequence/order, не hash/page/worker order.
- Hull wake учитывает old/new affected volumes и отдельные motion/debris snapshots.
- Rubble читает dirty X/Z pages, а не всю историю.
- Join перестраивает только changed keys; обычные transfers не вызывают full rebuild.

Полные builders остаются в коде как oracle и fallback для импорта, большого dirty
ratio и восстановления. Это strangler-подход: потребители переводятся по одному,
а не одним рискованным rewrite всего движка.

## 4. Два разных результата, которые не надо смешивать

### 4.1 CPU plateau без изменения геймплея

Cold pages + persistent indexes + due schedule + mutation journal могут сохранить
каждый логический обломок и каждую механику, устранив ежетиковый O(N). Это главный
и наиболее безопасный глобальный результат.

Canonical N и объём сохранения при этом могут продолжать расти. Точные записи
можно упаковывать, но нельзя обещать bounded memory без дополнительной модели.

### 4.2 Меньше логических/физических объектов

Если после этого память, snapshot или узкие точные контакты остаются проблемой,
нужен отдельный формат sparse/compound piles:

- произвольная точная занятость вместо ограниченной 4³ mask;
- материалы/paint/ledger и пустоты сохраняются;
- локальный split при wake/разрушении;
- явные правила contact order и reclaim.

Это уже может изменить поведение и требует новых golden. Оно не должно блокировать
CPU plateau из раздела 4.1 и не должно маскироваться под «простое кэширование».

## 5. Приоритеты глобального внедрения

### Этап 0 — быстрые доказательства инвалидации

- Merge-aware memo после transfers с warm/cold oracle.
- Persistent полный `Settled` с exact `want` gate.

Это не конечная архитектура, а проверка двух главных механизмов на реальном бое.

### Этап 1 — общий фундамент

- Stable-ID/order abstraction и центральный mutation journal.
- Diagnostic dirty/touched/fallback/bytes во всех потребителях.
- Cold/hot storage без смены snapshot и contact order.

После этого новые потребители не создают собственные несовместимые dirty-кэши.

### Этап 2 — перевести retained consumers

1. `Settled` и wake;
2. join membership/heads;
3. rubble columns/pages;
4. render/nav publication.

Каждый переход самостоятельно проходит oracle, 36 полей, replay, 1/4/8 и ABBA.
Fallback старого builder остаётся до доказанной стабильности.

### Этап 3 — ограничить оставшиеся растущие ресурсы

- Packed cold storage и более компактные snapshots;
- sparse/compound piles только отдельным ADR при доказанной необходимости;
- full drain до CPU и объяснённого memory plateau.

### Независимые направления, не заменяющие Resting World

- Exact bulk explosions — редкие пиковые фризы.
- Owned SIM publication/owner — frame pacing, не SIM mean.
- Gas froxels/empty hierarchy — GPU frame cost.
- Local motion/nav broadphase — масштаб юнитов, не текущий P0 при 62.

## 6. Что глобально не даст решения

- Новый physics middleware или float/GPU solver вместо `Fx`.
- Больше worker threads на тех же O(N) проходах.
- Общий active-list без центральной инвалидации.
- Persistent `Settled` как единственный кэш при полных join/rubble/wake.
- Merge-aware memo как доказательство bounded history.
- Удаление debris, запекание в terrain, LOD коллизий или снижение разрушения.
- Одновременный большой rewrite без старого oracle и поэтапной приёмки.

## 7. Критерий успеха всей стратегии

При одинаковом бое и тех же игровых состояниях:

- поздний тик не растёт пропорционально общему числу спящих записей;
- спокойный drain после `flying=0` выполняет только scheduled/query work;
- transfers не вызывают полную join-группировку;
- bytes read/write и fallback rate ограничены фактическими изменениями;
- 36 полей, ledger, snapshots/replay и physics 1/4/8 совпадают;
- native уменьшает SIM debt и frame tails, а не только diagnostic phase time.

Если cold/persistent архитектура не удерживает рост стоимости, тогда sparse/compound
piles становится следующим product-level решением, а не бесконечная серия кэшей.

## 8. Практическая рекомендация

Не начинать с compound physics и не разворачивать Resting World одним изменением.
Merge-aware memo принят (ADR-825), а persistent `Settled` реализован как точный
прототип, но отклонён по реальной цене обновления: см.
[449](449-persistent-resting-world-rejected.md). Поэтому этот документ остаётся
стратегической моделью, а не доказательством выигрыша persistent index. Следующие
срезы должны начинаться с измеренного consumer и включать все внешние mutation
paths; полный cache-friendly builder нельзя заменять только по алгоритмическим
соображениям.
