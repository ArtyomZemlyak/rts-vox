# 600 — Biped world anchors and shared motion preview

Date: 2026-11-14. This healthy-biped anchor iteration is accepted and delivered.
Existing uncommitted work and the user's queue remain.

## Report review and selected defect

Reports [593](593-planted-anchor-and-motion-window-review-20261113.md),
[594](594-moving-support-and-debris-contact-review-20261113.md), and
[595](595-support-readiness-and-acceptance-review-20261113.md) prioritize silent
anchor changes before the general SupportRef/Ready contract. Delivery
[599](599-multiped-world-anchors-20261114.md) closes the intact multiped lane-fit
path and explicitly leaves bipeds open. There is no report 600 preceding this
implementation; the recommendations are not treated as already implemented.

Fresh 300-tick full-SIM flat baselines in `out/walking600` reproduce:

| Baseline | Max consecutive planted slide mm/tick | Total mm |
|---|---:|---:|
| commander-before | 100.125 | 1133.368 |
| walker-before | 89.140 | 756.559 |

The strengthened motor regression also fails on the original implementation
at Commander tick 22. Its first mismatch is below the printed Fx precision;
the independent full-SIM traces establish the later visible displacement.
The failure includes world position, heading and normal, rather than relying
only on a planted counter or screenshot appearance.

## Implementation

- Extend the intact grounded support window to Commander and Walker 04,
  retaining at least one real reachable stance before admitting body motion.
- Keep the planted world contact and take-off point unchanged. The side/yaw
  guard refits the next landing goal. Initial placement still uses the authored
  stance width; airborne, damaged and externally shoved layouts retain their
  recovery path.
- Use the same two-leg coordinator in motor preview and deferred feet. Support
  alternatives, accepted movement and a short support wait share the existing
  geometry-derived turning cadence; the turn boost is applied once.
- Share stance-width selection across preview, height fitting and articulation.
  The previous height preview used 1.2 for the large Walker while actual feet
  used 1.0. The authored scale selection now agrees in all three places.
- Skip landing-goal refitting for an unchanged locked stance. No new save bytes,
  gameplay values, floating-point SIM operations or carrier assumptions are added.

The terrain floor remains the current scalar floor interface. This implements
the remaining healthy-biped anchor branch of 593, not SupportRef/Ready or a new
load/force solver. The nominal suspension and motor publication order remain.

Changed sources: `crates/sim-core/src/gait.rs` (`Foot::keep_walker_side`),
`crates/sim-core/src/range_motion.rs` (support eligibility, shared spread/cadence,
preview and alternatives), and `crates/sim-core/src/range_biped_support_tests.rs`
(exact stance invariance, actual soles, progress, pivot and restore).

## Checks and one rejected test assumption

`biped-before` is a valid failing baseline. In `biped-v1`, exact anchor, real
sole, alternating gait and restore checks passed for both models, but a new
speed assertion incorrectly demanded 90% of nominal top speed during Walker's
initial acceleration. The observed interval is 5129 mm in both the baseline
and the new full-SIM run. The assertion now compares physical travel against
the live commanded motor speed, requires actual progress, and still requires
at least 30 degrees of pivot. No gameplay speed was changed to satisfy it.

`focused-v2`: eight checks passed. After aligning the contact-alternative
preview and avoiding parked goal refits, the final source passed:

- `out/walking600/movement-final`: 52 SIM plus seven navigation checks.
- `out/walking600/forest-final`: two fresh SIM checks across bare/trees/rocks/mixed.
- Eight fresh 300-tick full-SIM runs, all with restore at tick 45 and physics
  and navigation workers 1/2. All report success and replay passed.

Final compile-input SHA256:
`e45ace6f31321060c2a30f7aafcdee630b5b1b9c9376393c8f7e253fb4d73b2e`.

| Final run under out/walking600 | Min planted | Max planted slide mm/tick | Planted IK error mm | Max root Y change mm/tick |
|---|---:|---:|---:|---:|
| commander-flat-final | 1 | 0 | 0 | 1 |
| commander-terrain-final | 1 | 0 | 0 | 38 |
| walker-flat-final | 1 | 0 | 0 | 1 |
| walker-terrain-final | 1 | 0 | 0 | 89 |

Spider and Strider flat/terrain were also rerun on the final source. All four
retain zero planted slide and planted IK error, with minimum supports 4/2.
CSV geometry is rounded to millimetres; the strengthened unit regression
compares stance contacts exactly in Fx and independently checks actual soles.

The Commander flat root change drops from 24 to 1 mm/tick. Walker remains at
1 mm/tick. Flat forward travel over ticks 50–79 remains 3750/5129 mm, and pivot
over ticks 110–159 remains 39.01/67.51 degrees, matching the two baselines.

Forest acceptance: Commander minimum speed 97.542434%, maximum loss per tick
0.122878 percentage points. Walker: 99.739854% and 0.013007 points. All eight
surface passes finish without blocked ticks. This preserves the user’s forest
limit and gradual resistance; it is not a new speed-balance change.

## Native evidence and delivery

Dev build: 63.45 s, source unchanged, SHA256
`42bdbd51382a25f58db2398d82cfa2055af94f1d6caf1e1ab5cb314cf82e97d0`.
Native Vulkan 90-frame realtime traversals and lightweight previews inspected:
`out/walking600/dev-commander` (terrain forward turn, zoom 1.4) and
`out/walking600/dev-walker` (terrain reverse, zoom 5.5). Whole-body poses include
the soles, knees and hull during the inspected transitions. Original PNGs are
preserved; they were not opened for the general visual pass.

One release build completed in 637.64 s with unchanged compile inputs.
Release SHA256:
`3b851289f3b12a07eae39bf35f68ae6a0f6c0a3ece2e0bfa3500d4e15a3c6af1`.
Both 90-frame realtime Vulkan release traversals passed and their lightweight
previews were inspected: `out/walking600/release-commander` and `release-walker`.
The logs include simulation-clock advancement; these are unpaused traversal
checks, not battle FPS evidence.

A fresh read-only shortcut copy confirms Полигон → play-range.cmd → play-range.ps1.
The actual launcher script selected the exact new hash with only Start-Process
intercepted. The same exe separately ran both release scenes; the shortcut
itself was not rewritten. `out/walking600/delivery-checks.json` records 45/45
passed checks: source/binary identity, final tests, eight replay/trace/anchor/IK
records, exact baseline control comparisons, both captures and launcher path.
All diagnostic game runs launched by this iteration have finished. The final
recovery ledger is `out/walking600/ledger.json`.

## Remaining scope

Support-source identity/revision, Ready after landing, moving/awakened debris,
damaged or impulsed anchor behaviour and Disabled support envelopes remain
open. Gate congestion, walking during explosions, and actual battle FPS also
remain in the original queue. The exact old folded Spider pose has not been
freshly reproduced. No broad force-transfer, carrier or battle-FPS completion
is claimed from these static/terrain traces and native traversal captures.
