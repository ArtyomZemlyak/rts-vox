# 457 — Native после ADR-827

3 ноября 2026. Артефакты: `out/optimization-457/native-current-2/`.

## Условия

World AI-vs-AI,60 секунд,Vulkan,3840×1949,focused=true,Windowed,AutoNoVsync,
saved tuning,compute4/physics4/navigation8/Rayon4. Обычный запуск без CorePhases.
`ValidForComparison=true`; `AcceptanceEligible=false`, потому что реального темпа20Гц
нет. Первая попытка была корректно отклонена runner из-за активных cargo/rustc;
после их завершения capture выполнен в новом каталоге.

Binary: `current-827-blast.exe`, SHA-256
`9BEBE7D87F2E92E51DEC3B5819011DE55CED6097F1AEEFD5EFDE188F8FE3705A`.

## Результат

| Метрика | ADR-827 native |
|---|---:|
|Средний FPS|26,50|
|Среднее кадра|37,696 мс|
|p50 / p95 / p99 кадра|34,678 / 78,774 / 110,856 мс|
|Худший кадр|142,245 мс|
|1% low|8,30 FPS|
|Тиков за capture|987|
|SIM realtime ratio|0,8202|
|Debt на выходе|19|
|Dropped|0|
|`simulation_core` p50 / p95 / p99|18,000 / 45,395 / 74,018 мс|
|`fragments` p50 / p95 / p99|4,872 / 19,084 / 21,048 мс|

Прежний native29,81 FPS был измерен в другом запуске и не является matched A/B.
Поэтому это новое фактическое состояние, а не доказанная регрессия или ускорение FPS.
Реального темпа20Гц по-прежнему нет; debt19 подтверждает ограничение SIM.

## Приоритет

Headless cumulative остаётся в [454](454-cumulative-827-measurement.md):
mean−10,82%, позднее окно−14,17%, debris−21,53%. Следующий самостоятельный этап —
owned SIM publication/owner для frame pacing; он не заменяет дальнейшую работу по
retained-history стоимости SIM и не обещает ускорение самого тика.
