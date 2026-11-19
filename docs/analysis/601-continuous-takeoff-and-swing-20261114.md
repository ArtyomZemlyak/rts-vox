# 601 — Continuous take-off and the departing leg

Date: 2026-11-14. The measured take-off/swing correction is accepted and delivered.
Broader support and in-flight limitations remain explicit below.

## Gap found in the previous acceptance

Reports 593–595 require checking the actual support/transition, not just a phase
flag. Iterations 599–600 fix the stance anchor and saved take-off origin. Their
acceptance did not separately measure the published contact at liftoff.

The last lane-fit in `Foot::step_supported` still reprojected a swinging contact
and forced its heading to the hull, including at progress zero. Thus `from`
was immutable while the actual contact could jump away from it. The first new
unit regression reproduced both translation and yaw discontinuity on a fixed
flat floor. This is a concrete remaining transition defect, not SupportRef/Ready.

The baseline is derived from unchanged 600 CSVs with their original trace hashes
checked, not rerun simulations stamped as new evidence. The new reporter and
the baseline engine fingerprint are recorded in
`out/walking601/baseline-takeoff.json`.

| 600 baseline | Max planted → nonplanted contact displacement, mm |
|---|---:|
| Commander flat | 16.401 |
| Walker 04 flat | 1.000 |
| Spider flat / terrain | 578.545 / 1319.167 |
| Strider flat / terrain | 483.017 / 667.612 |

These are discrete contact displacements. After a step begins to consume time
inside the current tick, nonzero displacement along its arc is legitimate;
it is not by itself evidence of a teleport.

## Implementation and rejected intermediate versions

1. **V1:** remove per-frame lane/yaw projection from admitted swings. Placement
   still fits the landing goal. Seven focused checks passed, and target liftoff
   displacement became zero in eight full-SIM runs. However Strider's physical
   IK residual at take-off remained 122/193 mm (flat/terrain). A zero target
   displacement was therefore insufficient acceptance: the hull had already
   moved beyond the departing leg's reach.
2. **V2:** start every new admitted swing with an elapsed arc sample. This made
   Strider's take-off IK error zero, but unnecessarily changed the cadence of
   reachable departures, especially Spider. It is not the selected version.
3. **V3:** preserve reachable progress zero and fit the earliest reachable arc
   sample only for an overextended departure. An extra departing-leg gate in
   the pre-suspension motor preview caused 28–33 percentage-point speed drops
   on rocks, despite passing bare/terrain acceptance. This extra gate is rejected.
4. **V4:** preserve the existing body admission and check actual departing-foot
   IK in every forest tick. This exposed a 7.27 mm error on Strider/rocks: an
   unfitted arc point is not the sole after terrain height/heading/normal fitting.
5. **V5 (selected):** take-off search and execution share `swing_contact`,
   including the occupied sole, interpolated heading/normal and lift-before-riser
   hold. Keep progress zero if that executed point is reachable. Otherwise
   search eight phase brackets and eight bisections for the first reachable
   executed sample within this tick's allowance and no farther than its apex.
   The saved origin is unchanged. The isolated Strider forest, reachable-origin
   and overextension checks pass; broader final qualification follows below.

The original retained-stance motor window remains in place. This iteration does
not add a complete post-suspension outgoing-leg/root transition proof; the
rejected approximate gate is not left enabled and the forest limit is preserved.

The first overextension fixture allowed too little phase time for the imposed
body translation. Its phase allowance was corrected to exercise a feasible
arc sample. The ordinary reachable-origin regression remains a separate check.
No balance values, save bytes, floating-point SIM code or new force model are
introduced. Goal fitting, live sole/terrain checks and legacy recovery remain.

Changed code: `gait.rs` (contact reach, shared executed trajectory, take-off timing
and the two regressions), `range_terrain_drag_tests.rs` (actual departing-foot
IK on forest surfaces), and the gait reporter plus its tests. The trial motor
gate was removed; it is not part of the selected implementation.

## Diagnostics

`scripts/gait_report.py` now reports both
`max_liftoff_displacement_mm` and `max_liftoff_ik_error_mm`. The first describes
movement; the second detects a desired point outside the actual links.
The existing four reporter checks pass, including the boundary between stance
slide and take-off displacement. Direct script invocation is required; an
initial unittest module invocation failed because `gait_report` was not on its
module search path, not because the diagnostic check failed.

CSV positions/residuals are rounded to mm. Without an event reason, a
planted → nonplanted transition could also mean support loss; the controlled
flat/terrain courses are interpreted together with their support counts and
the exact Fx regressions. This is not a general source/readiness event trace.

## Accepted mechanics and development qualification

Final compile-input SHA256:
`61cd71d53a1a1b5aa5083470fddad056b19d26af25b7facd39c13379e267de6a`.

- `movement-final`: 54 SIM + seven nav checks, final source verified.
- `forest-final-v5`: four SIM checks, sixteen bare/trees/rocks/mixed passes.
  Every planted-to-nonplanted transition also checks the solved ankle against
  the contact, with the existing 5 mm mechanics tolerance. All courses finish,
  zero blocked ticks; worst speed 93.097895%, worst drop 0.345105 percentage
  points/tick. The rejected braking gate is not present.
- `reporter-tests.json/log`: four passed Python checks, reporter/test hashes saved.
- Eight fresh 300-tick `final-{kind}-{flat|terrain}` runs with restore at 45
  and navigation/physics workers 1/2 all pass. Each retains its baseline
  number of liftoffs and minimum planted count (bipeds 1, Strider 2, Spider 4).
  All planted slide, planted IK error and take-off IK residual metrics are 0 mm.

| Final type | Take-off displacement flat / terrain mm | Take-off IK error mm | Root max dy flat / terrain mm |
|---|---:|---:|---:|
| Commander | 0 / 0 | 0 | 1 / 38 |
| Walker 04 | 0 / 0 | 0 | 1 / 89 |
| Spider | 0 / 0 | 0 | 72 / 175 |
| Strider | 160.608 / 199.812 | 0 | 1 / 68 |

Strider's displacement is a reachable, lifted first arc sample, rather than
its former lane snap of 483/668 mm. Its maximum IK residual over **all** phases
is now zero in both full-SIM runs; V1 left 122/193 mm. Spider terrain still has
up to 47 mm residual on other nonplanted swing samples. This iteration does
not claim every possible swing constraint is solved.

The entire body/heading/phase/up trace matches 600 exactly for Commander,
Walker and Spider. Strider flat forward/pivot remain unchanged; its last flat
X differs by 16 mm. On the hill, ticks 50–79 travel 9139 mm versus 9914 mm
(7.8% less); final X differs by 96 mm and max vertical change is 68 versus
74 mm. This is recorded rather than called an unchanged body trajectory.

Dev build: 64.45 s, source unchanged, SHA256
`2e8fd25d01308d4257c10945959e33616acef5ecde431dcb49259981e1f023f2`.
Inspected 90-frame unpaused/realtime Vulkan previews:
`out/walking601/dev-final-spider` (terrain turn, zoom 2.2) and
`out/walking601/dev-final-strider` (terrain reverse, zoom 0.9).
Original PNGs remain untouched; only companion previews were viewed.

## Release delivery

One release build completed in 638.36 s with unchanged compile inputs.
SHA256: `5f30821162a63e12cc3061ecf3617e43d7cdbadb152dd5d549d3455f12ed193b`.
Both 90-frame realtime/unpaused Vulkan traversals passed and their lightweight
previews were inspected: `out/walking601/release-spider` and `release-strider`.
These runs are not battle-FPS evidence.

Fresh read-only shortcut inspection confirms Полигон → play-range.cmd →
play-range.ps1. The actual selector chose this hash with only Start-Process
intercepted; the same exe separately ran both native release scenes. The
shortcut was not rewritten. `out/walking601/delivery-checks.json` records
59/59 checks passed: final source/binary/tests, eight replay/trace/stance/IK
records, take-off IK, unchanged liftoff counts, body controls, reporter hashes,
native captures and launcher selection. All diagnostic game runs have finished.
Recovery ledger: `out/walking601/ledger.json`.

General SupportRef/Ready, moving debris, damaged/impulsed/Disabled envelopes,
gate congestion, walking during explosions and actual battle FPS remain open.
The exact folded Spider pose in the old user image has not been freshly
reproduced; this report closes only the measured transition path.
