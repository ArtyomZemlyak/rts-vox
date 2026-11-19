# 664 — Побитово идентичное ускорение `material_flow` (`TileQueueSet`) и параллельной публикации `support_sources`

Дата: 2026-11-19. Пакет: оптимизация симуляции (`crates/sim-core/src/range_granular.rs`, `crates/sim-core/src/range_granular_motion.rs`, `crates/sim-core/src/burst.rs`, `crates/nav/src/vehicle/support.rs`). Итоговые артефакты: `out/round664/`.

## Профилирование остаточного пика `tick 587` (`~90 мс`) и фазовых сумм в ядерном сценарии `1100` тиков

Детальный замер всех фаз и субфаз `step_observed_all_tasks` на сценарии `4096×192×2048` (`1100` тиков, `physics_workers = 4`, `nav_workers = 2`, `out/round664/nuclear-before26/run.log` и `run-detail.log`) выявил два главных источника затрат:

1. **`material_flow` (`4052.44 мс` суммарно, пик `33.92 мс` именно на `tick 587`):** сам перенос зёрен `step_moist` ограничен `scans_per_tick = 4096` (`~0.39 мс`), а почти всё время тратилось в `Flow::observe`:
   - построение `BTreeSet<[u32; 3]>` на `~90 000` правок за тик;
   - `~405 000` вызовов `wake_tile` по 27-клеточному гало вокруг изменённых тайлов `4×4×4`, где `queued: Positions` (`Lookup<[u32; 3], ()>`) при достижении `1.84 млн` тайлов на `tick 587` вызывал рехеш многомегабайтной таблицы (`+23 мс` за один тик), а `revisit: BTreeSet<[u32; 3]>` выполнял сотни тысяч промахивающихся по кэшу вставок в дерево из `> 1 млн` узлов (`~10 мс` каждый тик).
2. **`rubble` → `physics_rubble_publish` (`6863.87 мс` суммарно по `rubble`, из них `2432.39 мс` в `support_sources`):**
   - `bounds.clear()` вызывался непосредственно перед `support_sources`, сбрасывая уже вычисленные параллельно в `classify` ограничивающие параллелепипеды `bounds[i]` и заставляя один поток заново считать `f.bounds()` для всех `~20 000` фрагментов каждый тик;
   - `support_sources` выполнялся строго последовательно на одном потоке с ежетиковой аллокацией `vec![false; len]`;
   - `Sources::new` безусловно вызывал `sort_unstable_by_key` по массиву `224`-байтных структур `Piece` (`~4.5 МБ`), хотя `fragments` уже упорядочены по возрастанию `id`.

## Реализованные побитово идентичные изменения

1. **`TileQueueSet` + `patch_scratch` в `Flow` (`range_granular.rs`, `range_granular_motion.rs`):**
   - `queued` и `revisit` объединены в лениво выделяемый по `X`-слоям битовый набор `TileQueueSet`, где каждое слово `(u64, u64)` хранит 64 тайла вдоль оси `Z` (`y * tz6 + (z >> 6)`, бит `z & 63`). Одно обращение к слову обрабатывает и постановку в `queued`, и отметку `revisit` (`wake_or_revisit`), и завершение тайла (`finish_tile`) без хеширования, рехешей и узлов `BTreeSet`.
   - Обход `for_each_revisit` при сохранении снимка идёт по возрастанию `(x, y, z >> 6, bit)`, выдавая последовательность `[x, y, z]`, на 100% идентичную итератору `BTreeSet<[u32; 3]>`.
   - В `Flow::observe` временное дерево `BTreeSet` заменено переиспользуемым буфером `patch_scratch: Vec<Pos>` с `sort_unstable() + dedup()`, сохраняющим канонический порядок активации FIFO.
2. **Параллельный `support_sources_with_bounds` и проверка упорядоченности в `Sources::new` (`burst.rs`, `nav::vehicle::support`):**
   - `support_sources_with_bounds` вызывается до `bounds.clear()`, переиспользует готовые `bounds[i]` и битовую маску `stacked_mask` из `RubbleScratch`, использует 64-битную маску податливых материалов `yield_low` и при `fragments.len() >= 1024` строится параллельно чанками `par_chunks(256)` на пуле физики с последующей конкатенацией в исходном порядке.
   - В `Sources::new` перед `sort_unstable_by_key` добавлена линейная проверка `windows(2).all(|w| w[0].source.id <= w[1].source.id)`.

Формат сохранений, лимиты за тик, порядок очередей и `data/*.ron` не менялись.

## Проверки и независимые оракулы

| Выборка | Результат | Отчёт |
| --- | ---: | --- |
| Целевые тесты `granular + rubble + support_sources`, включая два новых независимых оракул-теста (`tile_queue_set_and_sorted_patches_match_legacy_btree_and_positions_oracle` и `parallel_cached_support_sources_match_serial_reference_oracle` на `workers = 1, 2, 4, 8`) | `47/47` (`0` skip) | `out/round664/checks-focused/test-index.json` |
| Расширенный набор `craters + movement-quick + supports + walking-blasts + architecture + docs` (`sim-core 100`, `nav 21`, `modding 2`, `architecture 12`, `docs 3`) | `138/138` (`0` skip) | `out/round664/checks-suites/test-index.json` |
| Холодное восстановление ядерной волны `tick 265 → 620` (`2.16 млн` тайлов в очереди `Flow`) для `workers = 1, 2, 8` против `W4` | `100%` совпадение `removed`, `material_flow` и `state_hash` (`52c7bbf02613d85c`) | `out/round664/cold-restore-proof.json` |

## Изолированный 4-прогонный бенчмарк `ABBA` (`1100` тиков, `4096×192×2048`)

Все 4 запуска (`before → after → after → before`, `out/round664/nuclear-abba-summary.json`) побитово совпали по финальному хешу `d61f7948e6bd9555` и `removed = 30258057`:

| Метрика (`1100` тиков) | `before` среднее | `after` среднее | Изменение |
| --- | ---: | ---: | ---: |
| Весь прогон `total_ms` | `28983.71 мс` | **`25262.30 мс`** | **`−12.84%`** (`−3.72 с`) |
| Средний тик `mean_ms` | `26.349 мс` | **`22.965 мс`** | **`−12.84%`** |
| `p95_ms` | `63.70 мс` | **`55.40 мс`** | **`−13.04%`** |
| `p99_ms` | `67.53 мс` | **`58.77 мс`** | **`−12.98%`** |
| Пик `max_ms` (`tick 587`) | `89.12 мс` | **`65.62 мс`** | **`−26.36%`** |
| Тиков `>= 75 мс` | `1` | **`0`** | Устранены полностью |
| Тиков `>= 50 мс` | `243` | **`147.5`** | **`−39.30%`** |
| Сумма `material_flow` | `4035.43 мс` | **`1508.59 мс`** | **`−62.62%`** |
| Пик `material_flow` (`tick 587`) | `33.46 мс` | **`6.56 мс`** | **`−80.41%`** (`5.1×` быстрее) |
| Сумма `rubble` | `6743.85 мс` | **`5288.33 мс`** | **`−21.58%`** |
