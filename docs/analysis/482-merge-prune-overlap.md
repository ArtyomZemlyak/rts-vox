# 482 — Overlap merge keys и wake wreck prune

3 ноября 2026, ADR-831. Артефакты: `out/optimization-482/`.

## Причина

`debris_merge_keys` читает фрагменты и строит join inputs, а wake-stale wreck prune
изменяет только `e.wrecks`. Раньше prune ждал весь merge, хотя wake donors уже известны.

## ADR-831

При большой истории wake-stale wrecks удаляются через `rayon::join` параллельно с
построением merge keys. Merged donors удаляются отдельным retain после transfers, как
и прежде. Два retain сохраняют итоговый набор и порядок wrecks; landed, moved heads,
ledger, fragment compaction и hash не меняются.

## Проверки

- 200 тиков: все36 нетайминговых полей совпали.
- Две ordinary ABBA серии по1600: все36 полей каждого тика совпали.
- Replay snapshot300→600 на physics1/4/8: `695327e0588d053c` во всех режимах.

## Ordinary AI-vs-AI

physics8/navigation8/Rayon4:

| Пара | Mean | Позднее окно1201–1600 | `debris`, mean |
|---|---:|---:|---:|
|1, после→до|26,094→25,627 мс (−1,79%)|36,266→35,562 мс (−1,94%)|9,270→9,170 мс (−1,08%)|
|2, до→после|25,644→25,805 мс (+0,63%)|35,769→36,003 мс (+0,65%)|9,126→9,290 мс (+1,79%)|

Сводно mean 25,869→25,716 мс (**−0,59%**), позднее окно 36,018→35,782 мс
(**−0,65%**). `debris` 9,198→9,230 мс (+0,34%), поэтому принят именно небольшой
результат полного тика, а не заявление ускорения каждой подфазы. Одна пара отрицательная;
эффект ограничен и не отменяет retained-history рост.

## Границы

Это не persistent journal и не изменение join membership. Полный merge keys по-прежнему
строится; устранено только ожидание wake prune. Native FPS и новый cumulative baseline
в этом срезе не измерялись.
