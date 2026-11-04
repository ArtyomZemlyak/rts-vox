# 483 — Hash-таблица changed keys

3 ноября 2026, ADR-832. Артефакты: `out/optimization-483/`.

## Причина

`Memo::changed` использует changed keys только для `is_empty` и membership `contains`.
BTreeSet тратил упорядоченные вставки и поиск, хотя порядок ключей не нужен. Sorted Vec
(433) уже был отклонён и не повторяется.

## ADR-832

Changed keys хранятся в `FxLookup<Key, ()>` с прежним deterministic Fx hasher.
Sorted и unsorted/hash diff возвращают один и тот же набор; merge groups, donor order,
transfers, ledger и hash не меняются. Единый `changed_insert` добавляет пустое значение.

## Проверки

- 200 тиков: все36 нетайминговых полей совпали.
- Две ordinary ABBA серии по1600: все36 полей каждого тика совпали.
- Replay snapshot300→600 на physics1/4/8: `695327e0588d053c` во всех режимах.

## Ordinary AI-vs-AI

physics8/navigation8/Rayon4:

| Пара | Mean | Позднее окно1201–1600 | `debris`, mean |
|---|---:|---:|---:|
|1, после→до|26,080→25,831 мс (−0,96%)|36,294→35,755 мс (−1,49%)|9,435→9,092 мс (−3,63%)|
|2, до→после|26,014→25,809 мс (−0,79%)|36,079→35,601 мс (−1,33%)|9,332→9,087 мс (−2,63%)|

Сводно mean 26,047→25,820 мс (**−0,87%**), позднее окно 36,187→35,678 мс
(**−1,41%**), `debris` 9,384→9,089 мс (**−3,14%**). Merge keys уменьшились
примерно на14%.

## Границы

Полное построение `Memo::current` и diff по-прежнему O(retained). Изменён только
контейнер membership, а не lifecycle join. Native FPS и новый cumulative baseline
в этом срезе не измерялись.
