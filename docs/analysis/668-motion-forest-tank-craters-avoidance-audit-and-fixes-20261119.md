# Round 668: Motion, Forest Tank, Craters & Avoidance Audit and Fixes (2026-11-19)

Analytical summary of the `claude-opus-5-5-high` (`opus-reviewer`) read-only audit and the deterministic fixes implemented without breaking save format compatibility.

## Implemented Fixes

1. **Forest tank smoothness (`P0-1`, `P0-2`, `P0-3`, `P1-5`):**
   - `crates/nav/src/vehicle.rs` (`Changed::page_side`) and `crates/sim-core/src/range_motion.rs` (`terrain_changed`): for tracked hulls (`rigs.is_empty()`), route invalidation uses `touches_segment_beyond` with `near = max(half, support) + fell.radius + PAGE_SIDE` so felling edits on the hull's own page do not wipe `path` every tick; `blocked_ticks`, `escape`, `hunt`, and `coarse` are preserved instead of being reset on terrain edits.
   - `crates/sim-core/src/drive.rs` (`stopping_speed_to`), `crates/sim-core/src/range_motion.rs` (`step`), `crates/modding/src/navigation.rs` (`straight_through_deg`), and `data/range-navigation.ron` (`straight_through_deg: 25`): non-final waypoints with turn angle within `max(straight_through_deg, pivot)` brake toward `corner_speed` on the next leg instead of braking to zero when rounded preview cannot be proved.
   - `crates/sim-core/src/range_motion.rs`: tracked hull `sway_of` acceleration input is clamped per-tick to `max(brake, accel) * drive_factor * TICKS`, and `uneven` checks `max_floor - min_floor > terrain_detail` instead of exact equality.

2. **Crater traversal (`P0`, `P1-a/b/c`, `P2-b/d/e`):**
   - `crates/nav/src/vehicle.rs` (`Shape::reach`, `contact_for`, `Hunt::start`, `Hunt::load`): `Hunt::start` stores planner lookahead margin in `Shape::reach` instead of widening `Shape::span`, keeping belt bridging (`2 * k <= s.span`) aligned between planner and motor.
   - `crates/nav/src/vehicle.rs` (`Pose::excess`, `Hunt::expand`), `crates/nav/src/vehicle/transition.rs` (`Policy::grade_slack`, `unbounded_grade_exit`), `crates/nav/src/vehicle/connection.rs` (`prefix_open`, `advance_with_endpoints`), `crates/modding/src/navigation.rs` & `data/range-navigation.ron` (`grade_exit_slack_mm: 100`): grade-exit transitions enforce monotonic `next.excess <= old.excess + grade_slack` (with unbounded grade-exit reserved for active `escape_left > 0` manoeuvres), removing the `margin = 0` clamp in `Hunt::start`.
   - `crates/phys/src/lib.rs` (`clamp_tilt`): damps only the outward radial component of angular velocity `c.w`, preserving restoring angular velocity.
   - `crates/sim-core/src/range_motion.rs`: `take_plan` resets `self.escape = 0` when a non-empty path is returned, and the unturned alternative mirrors `escape_left > 0 || current.steep` + `belly_clear`.

3. **Movement, collisions & avoidance (`P1-1`, `P1-2`, `P1-3`, `P2`):**
   - `crates/sim-core/src/range_motion.rs`: reaching the end of an old path no longer clears `destination`/`hunt`/`coarse` when `destination` lies outside the `4 * cell_units` arrival ring (`step`, `continue_coarse`, `apply_hunt`); tangent obstacle slides keep `hunt`, `coarse`, and `needs_recovery` intact; tracked `blocked_ticks` only resets when velocity exceeds the escape speed threshold; `traffic_hint_from` widens the passing gate to `width + margin` without shadowing `want`; `Traffic::of` treats pending-search ordered hulls as `moving`.

4. **Walking terrain alignment (`P1`):**
   - `crates/sim-core/src/range_motion.rs` (`apply_walking_terrain`): derives geometric `min_up` from `leg_limit`, `support_x/z`, and `chassis.max_tilt_pct` so planner terrain limits match motor limits.
   - `crates/modding/src/walker_rig.rs`: tightens `support_x_cm` / `support_z_cm` bounds to `50..=400`.

## Verification

- `cargo test --offline -p phys -p modding -p nav`: `modding` `109/109`, `phys` + `nav` (`176/176` in `nav`, including `crater_pit_width_beyond_span_is_steep_for_both_planner_and_motor`, `grade_exit_excess_enforces_monotonicity_and_slack`, `clamp_tilt_preserves_restoring_angular_velocity`).
- `python scripts/test-fast.py --suite movement-quick --timeout 360`: `sim-core-lib` `54/54`, `nav-lib` `7/7` (`0` skipped).
- `python scripts/test-fast.py --suite supports --reuse-passed --timeout 360`: `sim-core-lib` `22/22`, `nav-lib` `13/13` reused, `modding-lib` `2/2` (`0` skipped).
- `python scripts/test-fast.py --suite walking-blasts --reuse-passed --timeout 360`: `sim-core-lib` `5/5`, `nav-lib` `1/1` (`0` skipped).
- `python scripts/test-fast.py --suite craters --suite architecture --reuse-passed --timeout 360`: `sim-core-lib` `25/25`, `harness-test-architecture` `12/12` (`0` skipped).
- `cargo test --offline -p sim-core --lib -- drive::tests:: range_steering:: a_tank_pushes_through_the_forest_and_the_trees_fall_as_debris a_retargeted_hull_drives_on_and_takes_up_the_new_route hull_contact_`: `39/39` passed (`0` failed).
