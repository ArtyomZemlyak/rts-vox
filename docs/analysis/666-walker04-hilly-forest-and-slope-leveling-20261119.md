# 666 — Walker04 на холмистом лесу: устранение ~1с фризов, ускорение дебриса и стабилизация крена на склонах

## Проблема

1. При движении `Walker04` (`Order::Move`) по процедурным холмам с лесом наблюдались многократные жёсткие фризы длительностью `365–946 мс` на тик (`step_us max = 793–946 мс`, `over50ms = 22` за 550 тиков), хотя прежний `bench_surface_motion` (`Order::Pilot` на ровном полигоне `y = 16`) не фиксировал планировщика и склонов.
2. При ходьбе по холмам 15-метровый корпус `Walker04` сильно кренился (до `32.8°`), потому что он наследовал шасси ванильного `Strider` (`support_x_cm = 90`, `support_z_cm = 90`, `max_tilt_pct = 84`, `settle_tilt_pct = 50`, `inertia = 225`, `body_sway_pct = 18`).

## Корневые причины и исправления

### 1. Побитово идентичные оптимизации леса и холмов (`nav`, `voxel`, `sim-core`)
- **`Hunt` + `Connection` (`crates/nav/src/vehicle.rs`, `vehicle/connection.rs`):** при `crush_cost == 0` / `crush_cap == 0` (штатно в `data/range-navigation.ron`) поиск больше не требует `prepared_bare_upper` в `probe_pose_contact` и использует `prepared_floor_upper` — отбрасывая проходимые 8×8-тайлы под кронами в O(1) вместо сканирования `~28 900` колонок × `192` позы за тик.
- **`Probe::scan` + кэши (`crates/voxel/src/world.rs`, `crates/nav/src/vehicle.rs`):** добавлен `prepared_floor_tile_detail` (возвращает `bounds` и флаг `bare`), поэтому на наклонных холмах при `ceiling == bounds[0]` колонка отвечает в O(1), а колонки с деревьями читаются одним пакетным запросом `world.column` от `bounds[0] - 1`. Увеличен размер кэша страниц `PAGE_SLOTS: 128 → 512`, а `GroundCells` расширен до `256 × 4` с точечной инвалидацией изменённых ячеек в `Columns::forget` вместо полного сброса на каждый срубленный воксель.
- **`Probe::pose_crush_flag` (`crates/nav/src/vehicle.rs`, `crates/sim-core/src/range_motion.rs`):** для `Motion` (где проверяется только `crush > 0`) после первого найденного дерева дальнейшие тайлы под широким корпусом пропускаются по `prepared_floor_upper <= min_belly`.
- **`SettledShape` и коллизии дебриса (`crates/sim-core/src/burst.rs`, `blast.rs`):** предрасчёт осей `axes` в `SettledShape`, дедупликация повторных записей в `Settled::hit_impl`, `trailing_zeros` в `fragment_contact` / `fragment_motion_contact`, отсечение пустых частей в `UnitVoxels::any_solid_box` (`overlap_bounds`).

### 2. Стабилизация крена `Walker04` на холмах + аудит `walking-reviewer` (`Claude Sonnet 5.5 (High)`)
- В `data/walker-rig.ron` и `crates/modding/src/walker_rig.rs` (с `#[serde(default)]` и проверкой границ) заданы параметры корпуса `Walker04`: `support_x_cm: 240`, `support_z_cm: 200`, `max_tilt_pct: 99` (порог крена `~8.1°`), `inertia: 600`, `settle_tilt_pct: 15`, `body_sway_pct: 8`, подключаемые в `Motion::of` (`crates/sim-core/src/range_motion.rs`).
- По замечаниям ревью `walking-reviewer` закрыты 3 критических edge-case:
  - **B1/B2 (`shape()` и `ride_now()`):** при потере или подламывании ноги (`legs_lost` / `legs_buckled`) `terrain_tilt` и `r.min_up` ограничиваются штатным `sim.chassis.walking.max_tilt_pct` (`84`), чтобы повреждённый `Walker04` честно заваливался по `ADR-288` и сохранял проходимость при маршрутизации.
  - **B3 (`traction_load`):** при вертикальной стабилизации (`max_tilt_pct >= 95`) и живых ногах (`carried() && hull.load > 0`) тяга/торможение больше не падают вдвое на склонах `>14°` из-за отрыва верхней пары углов 240-см прямоугольника `supports`.
  - **P1 (`LegRig::retracted`):** результаты `retracted_candidates` предвычисляются в `rigs_of` (`crates/sim-core/src/gait.rs`), убирая `24` IK-вызова с `sqrt` на каждый промах подошвы в `retracted_neutral`.

## Результаты замеров и проверок

- **До/после на холмах с лесом (`Walker04`, `Order::Move`, 550 тиков, до правки крена — побитово идентичный `hash=0x896cb123e3cadbd1`):**
  - `wall_ms`: `23 224 → 4 976 мс` (`−78.6%`), `motion`: `19 061 → 788 мс` (`−95.9%`), `hunt`: `13 129 → 174 мс` (`−98.7%`), `terrain_pose`: `4 217 → 300 мс` (`−92.9%`).
  - `step_us max`: `793 052 → 24 882 мкс` (`793 мс → 24.9 мс`), **`over50ms`: `22 → 0`**.
- **Эталон `bench_surface_motion --kind walker04` (`ABBA`, 4 режима):** `1532` строки CSV (`sha256=4011fdb94772cd64…`) и снапшоты (`trees`, `mixed`) совпали побайтово до правки крена; `mixed ms`: `6013 → 4186`.
- **Новые регрессионные тесты и сьюты:**
  - `nav::vehicle` (`119/119`), включая `sloped_forest_pose_crush_flag_and_zero_cost_hunt_match_reference_oracle`.
  - `modding::walker_rig` (`1/1`), `sim-core` (`hull_query_tests::`, `unit_felling_tests::walker04_`, `walking_blast_tests::` `11/11`, включая `walker04_hilly_forest_move_matches_across_cold_worker_restores` с `min_up_y >= 0.99` и холодным восстановлением `W1/2/4/8`).
  - Расширенные сьюты: `movement-quick + supports + walking-blasts + architecture + craters + ai + docs` (включая `0` `--skip`).
