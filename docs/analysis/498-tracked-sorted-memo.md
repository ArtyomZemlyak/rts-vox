# 498 — Tracked sortedness в join memo

3 ноября 2026. Артефакты: `out/optimization-498/`.

## Причина и прототип

`Memo::changed` на каждом тике заново проверял `previous.windows(2)` и
`current.windows(2)`, то есть выполнял два дополнительных O(N) прохода перед
самим diff. Прототип хранит `previous_sorted` и `current_sorted` и обновляет их
только при push/update/start/finish. Условие строгое `<`, поэтому duplicate ID
по-прежнему выбирает hash fallback. Сами `changed_sorted`, `changed_hash`, ключи,
геометрия, membership и порядок не меняются.

Это устранение полных проверок, а не новый persistent spatial index и не изменение
join lifecycle. Stable IDs и snapshots не затрагиваются.

## Проверка 200 тиков

Все36 нетайминговых полей совпали с fat-LTO ADR-836. Ранняя серия:
mean10,524→10,256 мс, `debris`1,841→1,774 мс.

## Полные ordinary серии

| Пара | Mean tick | Позднее окно1201–1600 | `debris`, mean |
|---|---:|---:|---:|
|1, tracked→baseline|24,761→25,287 мс|33,770→34,487 мс|8,374→8,747 мс|
|2, baseline→tracked|26,457→25,892 мс (−2,13%)|36,620→35,420 мс (−3,28%)|9,315→9,015 мс (−3,21%)|

Все36 полей каждого тика обеих серий совпали. Сводно baseline→tracked:
mean25,872→25,327 мс (**−2,11%**), позднее окно35,553→34,595 мс
(**−2,70%**), `debris`9,031→8,695 мс (**−3,73%**), `rubble`
3,276→3,174 мс (**−3,11%**). Обе пары улучшили полный тик.

Replay snapshot300→600 на physics1/4/8: `695327e0588d053c` во всех режимах.

## Native A/B

Четыре 60-секундных capture AB/BA против fat-LTO ADR-836, Vulkan, 3840×2160,
Foreground+Maximized, saved tuning, navigation8/Rayon4. Все
`ValidForComparison=true`, `AcceptanceEligible=false`, поскольку 20 Гц нет.

| Пара | Mean FPS | p99 кадра | `simulation_core` p50 |
|---|---:|---:|---:|
|1, tracked→baseline|33,156→32,955|92,861→93,828 мс|23,405→23,529 мс|
|2, baseline→tracked|32,853→32,884 (+0,10%)|94,927→93,986 мс|23,678→23,624 мс|

Сводно FPS 32,904→33,020 (**+0,35%**), p99 кадра 94,378→93,423 мс
(**−1,01%**), `simulation_core` p50 **−0,38%**, realtime ratio 0,9065→0,9085
(**+0,21%**). `fragments` p50 изменился на−0,51%.

## Решение

Изменение **принято как ADR-837**. Устранены только повторные sortedness-проверки;
join diff/fallback и gameplay прежние. Основной retained-history рост остаётся:
это ускорение текущих проходов, а не изменение их асимптотики всей системы.
