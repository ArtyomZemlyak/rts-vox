# 510 — Compact Settled selection

4 ноября 2026. Артефакты: `out/optimization-510/`.

## Причина и прототип

`Settled::around_bounds_into` материализовал `Vec<Option<([i32;3],[i32;3])>>` для
каждого фрагмента и затем читал corners повторно во всех восьми stripes. `out.boxes`
уже содержит те же world bounds. Прототип хранит только компактные `u32` indices.

`SettledCells::build` получает прежний bounds accessor, поэтому membership, requests,
cell links и canonical source order не меняются. Shape assignment обходит только
selected indices вместо полного `entry.is_some() || asks` скана. Это не persistent
index и не изменение queries.

## Проверки и результат

Первый вариант выявил потерю moving `asks` shapes. Union `selected || asks` был
исправлен через bit mask; после него все36 полей совпали.

Общий compact selection: mean+0,68%, `debris`+3,91%, `rubble`−9,80%.
Support-only вариант с исходным Option-builder для debris: mean+0,28%,
`debris`+0,09%, `rubble`+3,12%. Обе серии каждого варианта сохранили все36 полей.
Раннее небольшое улучшение support-only не подтвердилось полным боем.

## Решение

Оба варианта **отклонены и полностью откачены**. Compact functions, selection mask
и test call sites восстановлены к исходному Option-builder. ADR-838 сохранён.
Локальное улучшение rubble в первом варианте не используется как основание для ADR.

После отката отдельный restored release прошёл 200 тиков: все 36 нетайминговых
полей совпали с ADR-838, расхождений нет (`out/optimization-510/restored-200.csv`).
