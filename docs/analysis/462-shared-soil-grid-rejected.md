# 462 — Shared `soil_columns` grid: отклонено по расхождению gameplay

3 ноября 2026. Артефакты: `out/optimization-462/` и
`out/optimization-443/{after,restored}-462.*`.

## Гипотеза

`nuclear_soil_column` зависит только от координаты, `at` и config волны. Прототип
заменил её вычисление для каждой X/Z колонки каждой волновой клетки на один exact
grid на `Explosion`.

## Что прошло

19 contact-read тестов, включая прямой terraced scalar reader. Это не оказалось
достаточным для приёмки полного эпизода.

## Что отклонено

На первом impact-тике 373 появилось расхождение hash. Итог прототипа:

- hash `cefe965ad6c41c67` вместо `ae3798a582873796`;
- removed 9 887 948 вместо 9 505 559;
- `read_ground` 173,537→427,676 мс;
- claims559,703→601,401 мс.

Изменение gameplay и замедление однозначно запрещают принятие. Независимо от причины
расхождения, grid удалён полностью вместе с test changes.

## Восстановление

ADR-827 восстановлен. Restored release:

- hash `ae3798a582873796` совпал;
- removed 9 505 559 совпало;
- ledger сбалансирован.

Строгий cumulative остаётся в [454](454-cumulative-827-measurement.md).
