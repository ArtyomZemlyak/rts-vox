# 495 — Sorted contiguous Settled cells

3 ноября 2026. Артефакты: `out/optimization-494/`.

## Причина

Свежий [профиль494](494-current-3000-profile.md) показал 5,033 мс cell construction
и4,256 мс wake в позднем окне. Linked builder делал hash entry на каждую связь;
contact lists читались через отдельные nodes.

## Прототип

Каждая per-tick структура по-прежнему строится заново из тех же `selected` и `want`.
Ссылки `(cell, source)` сортируются по обоим полям. Hash map хранит диапазон в общем
`Vec<u32>` один раз на уникальную ячейку; iterator идёт по непрерывному диапазону.
Сортировка по source сохраняет прежний canonical contact order. Membership, queries,
cell size, stable IDs, snapshots и gameplay не меняются. Это не persistent index и не
пропуск запросов.

Направление sort-based uniform grid описано в
[NVIDIA Particles](https://developer.download.nvidia.cn/assets/cuda/files/particles.pdf),
но локальное решение зависит от CPU-замера полного движка.

## Проверки

Release World AI-vs-AI, physics8/navigation8/Rayon4:

- 200 тиков против fat-LTO ADR-836: все36 нетайминговых полей совпали.
- Стандартный `cargo test -p sim-core settled_` не дошёл до запуска тестов из-за двух
  прежних ошибок компиляции в `fragment_join_memo.rs:187,194`: `assert_eq!` сравнивает
  `FxLookup`, не реализующий `PartialEq`. Это следствие контейнера ADR-832, не данного
  прототипа; чужой тест не изменялся. Release и игровой oracle доступны.

## Полные ordinary серии

| Пара | Mean tick | Позднее окно1201–1600 | `debris`, mean |
|---|---:|---:|---:|
|1, sorted→baseline|26,580→24,935 мс|36,993→34,309 мс|10,033→8,569 мс|
|2, baseline→sorted|25,100→26,577 мс (+5,88%)|34,508→36,819 мс (+6,70%)|8,640→10,040 мс (+16,21%)|

Все36 полей каждого тика обеих серий совпали. Сводно baseline→sorted:
mean25,018→26,578 мс (**+6,24%**), позднее окно34,408→36,906 мс
(**+7,26%**), `debris`8,604→10,037 мс (**+16,65%**), `rubble`
2,976→3,125 мс (+5,00%). `motion` практически не изменился (−0,21%).

## Решение

Прототип **отклонён и откачён**. Сортировка `(cell, source)` и дополнительные
entries стоят дороже сэкономленных hash entries; улучшения полного тика нет.
Исходный linked builder, wide links, scratch semantics и тесты восстановлены.
Restored release повторно прошёл 200 тиков с нулём расхождений по всем36 полям.
ADR не создаётся, replay и native не запускаются при отрицательном headless
результате. Внешняя sort-based grid литература не заменяет локальный CPU-замер.
