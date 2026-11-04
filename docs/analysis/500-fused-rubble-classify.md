# 500 — Слияние rubble classify и Settled boxes/ranges

3 ноября 2026. Артефакты: `out/optimization-500/`.

## Причина

[Профиль496](496-rubble-3000-profile.md) показал 6,022 мс support build в rubble.
Перед ним classify уже проходил всю историю и вычислял local bounds. Второй
`Settled::around_bounds_into` затем снова проходил все фрагменты для world boxes и
support ranges. Это отдельная повторная работа, а не query-driven builder 465.

## Прототип

Общий `Settled::finish` принимает готовые ranges. Обычный debris путь вызывает его
после прежнего `around_bounds_into` без изменения. Rubble classify теперь в том же
batch создаёт:

- прежние local bounds для columns/support;
- world boxes для `Settled`;
- support ranges только для фактически stacked записей.

Batch order, `want`, candidates, cells, shapes, support queries и contact order
сохранены. Нет persistent state, superset, sorting или изменения gameplay.

## Полные ordinary серии

200 тиков и обе полные серии сохранили все36 нетайминговых полей.

| Пара | Mean tick | Позднее окно | `debris`, mean | `rubble`, mean |
|---|---:|---:|---:|---:|
|1, fused→baseline|24,816→24,729 мс|34,047→33,822 мс|8,488→8,351 мс|2,907→2,952 мс|
|2, baseline→fused|24,782→24,897 мс (+0,46%)|34,022→34,139 мс (+0,35%)|8,459→8,554 мс (+1,12%)|2,960→2,898 мс (−2,08%)|

Сводно baseline→fused: mean24,756→24,856 мс (**+0,41%**), позднее окно
33,922→34,093 мс (**+0,51%**), `debris`8,405→8,521 мс (**+1,37%**),
`rubble`2,956→2,903 мс (**−1,81%**). Локальное улучшение rubble не окупило
остальную стоимость fused builder.

## Решение

Прототип **отклонён и откачён**. Исходный classify и `around_bounds_into`
восстановлены. Restored release повторно дал нулевые расхождения по всем36 полям
на200 тиках. ADR, replay и native-замер не нужны при регрессии полного тика.
