# 440 — Dirty membership для join current: отклонено

1 ноября 2026. Артефакты: `out/optimization-440/`.

## Гипотеза

После ADR-825 группируются только changed keys, но `Memo::current` по-прежнему
строится по всей истории. Прототип клонировал прежний membership и обновлял dirty
IDs от flying/wake/merge/emit. Slot после compaction восстанавливался binary search
по stable ID. Reclaim/sweep и несортированный вход использовали full rebuild.

## Корректность

Направленные проверки прошли:

- warm/cold после merge и изменений paint/position;
- sorted/hash/unsorted changes;
- lab fragment landing и snapshot restore;
- release-сборка;
- все 36 нетайминговых полей каждого тика matched A/B и hash `ef96977a5b55489b`.

Полный suite был остановлен после нескольких боевых сбоев. Контрольный
`player_artillery_surface_blast_keeps_ground_energy_and_replays` даёт68 вместо59
и на кандидате, и при принудительном full rebuild, и на восстановленном чистом
ADR-825. Поэтому он не считается регрессией кандидата и не исправлялся в этой
работе. Это не доказывает происхождение остальных сбоев suite.

## Измерения

Baseline `out/optimization-439/baseline-825.exe`, SHA-256
`ccfe075969c4c853a2a4b21398f7b6b9bd1aa0511cef9a053dc87beaed0cd991`.
World AI-vs-AI,1600 тиков,physics4/navigation8/Rayon4, порядок «после/до».

| Показатель | Baseline | После | Изменение |
|---|---:|---:|---:|
|Полный mean|31,963 мс|37,653 мс|+17,80%|
|Позднее окно1201–1600|45,476 мс|54,169 мс|+19,11%|
|`debris`|13,526 мс|19,139 мс|+41,49%|
|`merge_keys`, позднее окно|2,920 мс|11,394 мс|+290,3%|

## Решение

Прототип отклонён, рабочее дерево возвращено к ADR-825. Несколько тысяч dirty IDs
с ordered-set insertion, binary membership updates и lazy slot lookup оказались
дороже линейного cache-friendly построения current. Алгоритмическая меньшая работа
не равна меньшему времени: цена ветвлений, поисков и дополнительных структур имеет
решающее значение. Следующий persistent Settled не должен повторять эту модель без
предварительного замера стоимости журнала и mapping.
