# 484 — Восемь compute workers по умолчанию

3 ноября 2026, ADR-833. Артефакты: `out/optimization-484/`.

## Причина

Physics и navigation уже используют восемь workers, а render compute pool по умолчанию
фактически имел четыре. Увеличение глобального Rayon pool было отклонено (476), поэтому
изменяется только compute pool рендера, без изменения SIM, physics или navigation.

## ADR-833

`blast::compute::plugin(None)` теперь задаёт `compute.min_threads = compute.max_threads = 8`.
Явные `--compute-workers 1..=16` сохраняются; IO/async pools не меняются. Это фиксированная
конфигурация render compute, а не чтение числа ядер или изменение работы/геометрии.

## Native A/B

Один и тот же default-physics8 binary, меняется только compute4/8. Четыре 60-секундных
capture AB/BA, Vulkan, 3840×2160, Foreground+Maximized, saved tuning,
navigation8/Rayon4. Все `ValidForComparison=true`, `AcceptanceEligible=false`, поскольку
20 Гц не достигнуто.

| Пара | Mean FPS compute4→8 | p99 кадра | `simulation_core` p50 |
|---|---:|---:|---:|
|1|29,8→31,0 (+4,03%)|101,813→98,917 мс|13,659→11,559 мс|
|2|30,5→31,0 (+1,64%)|99,673→99,680 мс|12,531→11,998 мс|

Сводно FPS вырос примерно на **2,84%**, `simulation_core` p50 уменьшился примерно
на **11%**. Реального темпа20 Гц по-прежнему нет.

Отдельный запуск без override подтвердил исходный default `compute_workers=4`, поэтому
сравнение относится именно к текущей конфигурации, а не к искусственно заниженному baseline.

## Проверка default

Unit-тест default8 и независимости IO/async pools прошёл. Новый native executable
без override подтвердил `compute_workers=8`: 60 секунд, **31,7 FPS**, p99 **95,374 мс**,
1075 тиков, realtime ratio **0,8940**, `ValidForComparison=true`. 20 Гц всё ещё не
достигнуто, но default подтверждает и превосходит matched compute8 серии выше.
