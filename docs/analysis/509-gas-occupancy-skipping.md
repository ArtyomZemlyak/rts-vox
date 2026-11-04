# 509 — Gas display occupancy и empty-space skipping

4 ноября 2026. Артефакты: `out/optimization-509/`.

## Причина

GPU profile ADR-838 показал gas raymarch около3,8 мс на publication batch. DDA
читает каждую display cell, включая заведомо пустые. Empty-space skipping через
occupancy — стандартное направление ([SparseLeap](https://johanna-b.github.io/files/documents/sparseleap_vis17.pdf)).

## Прототип

Существующий display compute дополняется маской 2×2×2. Перед build маска очищается;
каждая display cell с `gas_may_show(matter)` отмечает свой блок. Raymarch пропускает
только нулевые блоки. Переход на следующий coarse boundary сохраняет DDA и число
small-cell шагов в пределах прежнего 360-step бюджета. Состав цвета, симуляция газа,
порядок вычислений непустых cells и progressive publication не меняются.

## Проверки и результат

Первые запуски последовательно выявили типовые ошибки WGSL: signed/unsigned
конструкторы, coarse face и индекс occupancy. Они были исправлены, после чего
capture стал валидным.

Четыре 60-секундных capture AB/BA против ADR-840:

| Пара | Mean FPS | p99 кадра | `fragments` p50 | `simulation_core` p50 |
|---|---:|---:|---:|---:|
|1, after→before|35,457→35,391|87,354→87,015 мс|2,974→2,871 мс|22,530→22,722 мс|
|2, before→after|35,750→35,518 (−0,65%)|86,251→86,146 мс|2,904→2,850 мс|22,879→22,754 мс|

Сводно FPS 35,571→35,487 (**−0,23%**), p99 кадра **+0,14%**, `fragments`
**+0,84%**, `simulation_core`−0,70%. Realtime ratio изменился на+0,09%, что не
доказывает ускорение: обе пары по FPS отрицательные.

## Решение

Прототип **отклонён и полностью откачён**. Clear/build occupancy и проверки в DDA
не окупились. ADR-840 восстановлен, включая исходные display pipeline и 360-step loop.
Литературное направление empty-space skipping остаётся зависящим от плотности поля
и стоимости подготовки маски, а не автоматическим выигрышем.
