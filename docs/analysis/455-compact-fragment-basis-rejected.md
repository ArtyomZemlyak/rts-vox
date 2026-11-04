# 455 — Compact `Fragment::basis`: меньше record, но медленнее бой

3 ноября 2026. Артефакты: `out/optimization-455/`.

## Гипотеза

`basis: [V3;3]` занимает 72 байта в каждом `Fragment`, включая многочисленные
`IDENTITY`. Эксперимент заменил его на enum: identity inline, rotated basis в
`Box<[V3;3]>`. Цель — уменьшить hot record и cache traffic всей retained истории.

## Реализация и проверки

- Размер `Fragment` уменьшился со 184 до112 байт (−39,1%).
- Все конструкторы, save/load, tumble и test signatures сохранили прежние значения.
- `a_thrown_piece_tumbles_and_one_laid_down_does_not` прошёл.
- `stock_weapons_soil_balance_and_replay` прошёл.
- 200 и1600 тиков: все35 нетайминговых полей каждого тика совпали.

## Производительность

World AI-vs-AI,1600 тиков,physics4/navigation8/Rayon4:

| Показатель | ADR-827 | Compact basis | Изменение |
|---|---:|---:|---:|
|Mean|32,575 мс|34,485 мс|+5,86%|
|Позднее окно1201–1600|46,309 мс|50,621 мс|+9,32%|
|`debris`, mean|13,652 мс|15,290 мс|+12,00%|
|`debris`, позднее окно|22,350 мс|25,809 мс|+15,48%|

Allocations и indirection для rotated записей стоят дороже уменьшения inline record.
Меньшее `size_of` не означает меньшее время: общий тик и целевая retained-фаза
ухудшились. Обратная серия не нужна при таком отрицательном результате.

## Решение

Эксперимент полностью откачен. `FragmentBasis`, `.into()` и test-only clones удалены;
ADR-827 восстановлен. Restored release на200 тиках дал 0 расхождений. Строгий
суммарный результат остаётся в [454](454-cumulative-827-measurement.md). Это не ADR и
не изменение принятого layout.
