# 665 — Устранение покадровых аллокаций в `spatial_ui` / `construction` и чистота сборки `sim-core` + `render` (`0` предупреждений)

2026-11-19. Продолжение работ по приоритетам `/goal` после `663` (механика хождения) и `664` (оптимизация `material_flow` + `support_sources`).

## Причина и проблема

1. **Покадровые кучные аллокации и лишние запросы в `construction::spatial_catalog`, `construction::refresh`, `spatial_ui::update` и `ui_motion::input`:**
   - После перевода цен рецептов в единый остров `construction::tooltip` (`660`) и переноса очереди в глобальный верхний `production_hud` (`617` / `660` / `661`) функция `construction::spatial_catalog` на каждом кадре при выбранном инженере или заводе продолжала вызывать `sim.production_quote` / `construction_energy_estimate`, форматировать 4 неиспользуемые строки `SpatialRecipe::cost`, строки `heading` и `hint`, а также собирать `plans: Vec<_>` и `queue: Vec<(RangeUnitKind, Handle<Image>)>`, хотя проекция читала только список `recipes` (`kind`, `image`) и булев признак активной работы (`state.motion.work`).
   - В `construction::refresh` в боевом и песочном режимах на каждом кадре выделялись `plans: Vec<_>` и `queue: Vec<_>` даже при отсутствии legacy-узлов очереди (`QueueWidget`, `QueueText`, `QueueImage`, `Caption::Heading`).
   - В `spatial_ui::update` статические подписи (`Caption::Toggle`, `Caption::CatalogToggle`, `Caption::Name(0..4)`, пустой `Caption::Tooltip`) конвертировались в `String` до сравнения с текущим `text.0`, а `ui_motion::input` форматировал `"UI эффекты: {}% · сменить"` на каждом кадре `Update` даже без смены процента.
2. **Побочный эффект раунда `662` в тестовой UI-фикстуре `construction::tests::fixture`:**
   - В `construction::tests::fixture` союзный `Tank` (`[2600, 16, 2800]`) стоял в `1000` вокселях (`100 м`) от единственного холостого вражеского `Tank` (`[3600, 16, 2800]`, радиус орудия танка `1200` вокселей). До `662` союзный танк стрелял фугасом сквозь пробитую полость и не успевал уничтожить врага за 200 тиков. После исправления `Auto -> Kinetic`, прицеливания по живому остатку ядра и `core_bore_pct: 15` (`662`) союзный танк уничтожал врага быстрее 200 тиков, переводя бой в `Phase::Victory` (`!production_open()`) до завершения 200-тикового заказа завода в `upcoming_groups_preserve_order_and_real_completion_advances_the_published_queue`.
3. **Предупреждения компилятора (`3` в `sim-core` + `29` в `render --bin blast`):**
   - В `sim-core` тестовые обёртки (`Foot::swing_contact`, `refresh_support`, `step_ready_traced`, `reproject_stance`, `keep_walker_side`, `Motion::advance_walking_feet`, `Front::pop_first`, `Front::after`) не были помечены `#[cfg(test)]`.
   - В `render` оставались неиспользуемые варианты `Part` / `Caption` / `Visual` удалённой дублирующей очереди `spatial_ui`, неиспользуемые константы палитры `panel::theme`, приватный `ui_motion::IntensityLabel` в `pub(crate)`-системе `ui_motion::acceptance::capture` и тестовые помощники без `#[cfg(test)]`.

## Изменение

1. **`crates/render/src/bin/blast/construction.rs` и `spatial_ui.rs`:**
   - `SpatialRecipe` сокращён до `{ kind, image }`, `SpatialCatalog` — до `{ recipes, has_work: bool }`; убраны покадровые вызовы `production_quote` и форматирование `cost` / `heading` / `hint` / клонирование вектора очереди.
   - В `construction::refresh` сборка `plans: Vec<_>` и `queue: Vec<_>` выполняется только при наличии подписанных узлов legacy-очереди/заголовка (сохраняя полное покрытие модульных тестов `construction::tests`), а в живой игре проверка планов стройки работает через безаллокационный предикат `.any(...)`.
   - В `spatial_ui::update` сравнение статических/заимствованных строк (`Caption::Toggle`, `Caption::Tooltip`, `Caption::CatalogToggle`, `Caption::Name`) выполняется по `&str` до выделения `String`; удалены мёртвые варианты `Part` (`Queue`, `Leader`, `Depth`, `Filter`, `QueuePip`, `Greben`, `QueueMeter`), `Caption` (`Heading`, `Hint`, `Queue`), `Visual::Queue` и `catalog_button`, а `Part::Production` / `Part::QueueRow` оставлены под `#[cfg(test)]` для контрактной проверки единственности верхней очереди.
2. **`crates/render/src/bin/blast/ui_motion.rs`:**
   - `IntensityLabel` получил видимость `pub(super)`, а `ui_motion::input` кэширует последний опубликованный процент в `Local<Option<u8>>` и не аллоцирует `format!` на спокойных кадрах.
3. **`crates/render/src/bin/blast/construction/tests.rs`:**
   - В не-боевой UI-фикстуре `fixture()` союзный `Tank` перенесён с `[2600, 16, 2800]` на `[1200, 16, 2800]` (`2400` вокселей до холостого вражеского `Tank`, вне радиуса автоатаки `1200`), чтобы проверка многосотентикового производства и улучшений `T1→T4` шла в штатной фазе `Phase::Fighting` без фонового обстрела.
4. **Нулевые предупреждения компилятора (`sim-core` и `render`):**
   - Добавлены `#[cfg(test)]` к тестовым помощникам в `gait.rs`, `range_motion.rs`, `range_felling_front.rs`, `spatial_ui.rs`, `optical_ui.rs` и `ui_acceptance/geometry.rs`; очищены неиспользуемые импорты, константы и переменные в `panel/hud.rs`, `panel/theme.rs`, `spatial_ui/chrome.rs`, `spatial_ui/projection.rs`, `ui_acceptance.rs`, `laboratory.rs` и `laboratory/visuals.rs`.

## Проверки и результаты

| Проверка | Результат | Артефакт |
|---|---|---|
| Предупреждения компилятора `sim-core` + `render --bin blast` (`sim-test` и `release`) | `0` предупреждений в обоих крейтах (было `3 + 29 = 32`) | `out/round665/release-build.json` |
| Наборы `ui-animation` + `smoke` (`gait::tests`, `architecture`, `docs`) | `101/101` passed (`35 + 51 + 12 + 3`), `0` failed, `0` skipped | `out/round665/checks-suites/test-index.json` |
| Целевые модули `spatial_ui::`, `ui_acceptance::`, `construction::tests::` (включая новые регрессионные тесты `spatial_catalog_tracks_work_state_for_factory_and_engineer` и `intensity_label_settles_without_idle_writes_and_cycles_on_press`) | `41/41` passed, `0` failed, `1` явный skip | `out/round665/checks-spatial-construction/test-index.json` |
| Обновление финального `target/release/blast.exe` (включает `662 + 663 + 664 + 665`) | `90 880 000` байт, `sha256=4180210387b634f59bb7d7b82be3731b9d52b6c191a3f298349b8c978e45bf7c` | `out/round665/release-build.json` |

Явный `--skip`: `construction::tests::all_stock_scenarios_offer_only_their_live_allied_builders_recipes` (прежняя тяжёлая full-world проверка всех стоковых сценариев, согласно `AGENTS.md`).

## Done / Remaining

| Статус | Работа |
|---|---|
| Done | `662`: пробой ядра насквозь (`core_bore_pct: 15`), добивание живого кластера ядра (`aim_point_from` + `verified_world_pos`), `Auto -> Kinetic` и `MotorScratch` (`motion_commit −20.42%`) |
| Done | `663`: `step_sole_climbable` по плоскому максимуму 9 проб подошвы, ближний коридор ретракции `(1/4, 0/1)`, анти-реверс голени в `clamp_arriving_reach` (`scene 16`: `+34.9%` пути, `reversed = 0`, `6/6` обычных сцен byte-equal) |
| Done | `664`: побитово идентичный `TileQueueSet` в `material_flow` (`−62.62%` суммы, пик `33.46 → 6.56 мс`) и параллельный `support_sources_with_bounds` (`rubble −21.58%`, Nuclear 1100 `total −12.84%`, `over_75ms: 1 → 0`) |
| Done | `665`: устранение покадровых аллокаций `spatial_catalog` / `spatial_ui` / `ui_motion`, поправка дальности танков в `construction::tests::fixture`, `0` предупреждений `sim-core` + `render`, пересборка `target/release/blast.exe` |
| Remaining | Масштабирование `128 / 1000 / 5000` юнитов с проверкой SIM debt и сходимости `workers 1/2/4/8`; дальнейшее сглаживание походки на изрезанном рельефе (`623`/`628`); foreground FPS приёмка (`627`) |
