# 494 — Профиль ADR-836 на 3000 тиков

3 ноября 2026. Артефакты: `out/optimization-494/`.

## Методика

World AI-vs-AI, 3000 тиков, fat-LTO ADR-836, physics8/navigation8/Rayon4.
Endpoint hash `e1b57d109401f0e7` совпал с предыдущим профилем. Игровые counters
и число фрагментов во всех окнах совпали; это профиль состава работы, а не новый
matched A/B процент ускорения.

## Рост полного тика

| Окно | Тик | `debris` | `rubble` | `motion` | Среднее fragments |
|---|---:|---:|---:|---:|---:|
|1–1600|25,895 мс|9,056 мс|3,201 мс|5,071 мс|135 658|
|1601–2400|40,816 мс|19,942 мс|7,345 мс|6,575 мс|325 730|
|2401–2700|46,350 мс|24,304 мс|8,969 мс|6,876 мс|402 365|
|2701–3000|49,160 мс|26,570 мс|9,961 мс|6,425 мс|454 113|

Flying остаётся около3,5 тыс.; рост ведёт retained history. В последнем окне
`debris` делится на Settled12,344, merge keys5,076, wake4,256, compact1,382,
translate1,084, confirm1,030 мс. Внутри Settled: clear1,362, boxes1,587,
ranges0,259, queries0,881, candidates2,717, cells5,033 мс.

## Следующий глобальный кандидат

Cell construction — крупнейшая отдельная стадия. Текущий builder выполняет hash
entry на каждую связь и читает контакты через linked nodes. Проверяется временная
(per-tick) sorted contiguous структура: сортировка `(cell, source)`, один hash entry
на уникальную ячейку, непрерывный source order. Это не persistent index, не query
culling и не изменение membership. Sort-based uniform grids являются стандартным
направлением ([NVIDIA, Particles](https://developer.download.nvidia.cn/assets/cuda/files/particles.pdf)),
но решение принимается только по CPU AB/BA и точному oracle этого движка.
