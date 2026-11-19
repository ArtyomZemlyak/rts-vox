# 599 — Multiped world anchors (2026-11-14)

Status: this intact-multiped stable-ground iteration is accepted and delivered.
The broader support/Ready work remains open as listed below.

## Fresh baseline

The current Spider flat full-SIM run is `out/walking599/repro-flat`.
It records 360.723 mm maximum consecutive-planted slide, 3724.216 mm total,
four minimum planted feet and zero planted IK error. Root Y changes by at most
63 mm/tick. The much larger jumps in older reports are not this fresh repro.
The supplied screenshot alone does not establish the same folded pose in 598.

The new deterministic regression starts, stops, pivots, reverses, turns while
moving and settles both Spider and Strider. It failed before anchor locking at
Spider tick 25, leg 6: a planted contact moved 82.5 mm without liftoff.
The first attempted baseline build was rejected by the validator because the
source changed during compilation; only `anchor-baseline` is the valid repro.

## Implementation in qualification

- Separate placement-limited IK from contact IK. SIM articulation and native
  interpolation solve the authoritative contact, without silently clamping it
  into another point. Existing placement callers retain their lane checks.
- Healthy grounded multipeds without an external shove retain each stance's
  world position, heading and normal until liftoff. Damaged/airborne/impulsed
  paths retain their existing recovery behaviour.
- Generalize motor support admission to intact radial legs. Admission precedes
  hull publication and previews the same coordinated feet as articulation.
- Preserve Spider's bounded stance-height fitting. Four-legged admitted poses
  use only the actual link limit after suspension, avoiding a duplicate wider
  reserve fit. Turning admission also respects the stance corridors.

## Rejected iterations and remaining acceptance

`narrow-v2`: 32/37 passed; thin rails stalled, a hill lost a real sole, and
forest speed regressed. `narrow-v3`: 33/37 passed; pure contact IK fixed the
forward hill, but radial descent admission, reverse reach and an articulated
siege walker's turn still needed correction. Neither is accepted evidence.

`focused-v4`: 6/7 passed; only the 2.8 m rail still stalled. Its saved diagnostic
shows four repeating swings while their supporting partners remain on the
ledge. Touchdown was incorrectly tested with the placement-clamped IK. Terrain
fitting changes local X/Z when the hull leans; pure contact IK now accepts the
reachable landing and releases the partner for its step. `focused-v5`: 7/7.

The gait diagnostics were then aligned to the same contact solver. This changes
their reported IK metric, not the simulation state: flat hashes before/after
the diagnostic correction both equal `70885a44e8058869`.

## Accepted mechanics and development evidence

Compile-input SHA256:
`ef0ff656b536999ccfa0dc4158a18dd5402ee7ebb79b0256f4b0672fd2106514`.

- `out/walking599/movement-final`: 52 SIM and 7 nav checks passed, including
  biped support, inertia, real soles, leg intersections, dirty terrain/rubble,
  and serial versus prepared/deferred motion across workers.
- `out/walking599/forest-final`: two fresh SIM checks passed on the final
  source. Spider minimum speed is 99.373922%; largest loss per tick is
  0.031304 percentage points. Strider: 93.097895% and 0.345105 points.
- Four fresh 300-tick full-SIM runs use restore at tick 45 and workers 1/2 for
  both navigation and physics. All report `success` and `replay_passed`.

| Run under `out/walking599` | Minimum planted | Max planted slide mm/tick | Planted IK error mm | Max root Y change mm/tick |
|---|---:|---:|---:|---:|
| flat-contact-final (Spider) | 4 | 0 | 0 | 72 |
| terrain-contact-final (Spider) | 4 | 0 | 0 | 175 |
| strider-flat-final | 2 | 0 | 0 | 1 |
| strider-terrain-final | 2 | 0 | 0 | 74 |

The Spider's flat height change is 72 mm, versus 63 mm in this fresh baseline;
this iteration fixes stance drift, not all residual suspension motion. Maximum
IK error over **swing** contacts remains 51/47 mm for Spider and 122/193 mm for
Strider (flat/terrain). A stored swing point is not a load-bearing sole.
These traces and the screenshot inspection do not close general Ready/source
semantics from 593–595, nor reproduce the exact old folded pose from the image.

Development build: 85.19 s, unchanged source fingerprint, binary SHA256
`a0477482b9b7968a7b1fe8de12291e18260bffcd14f1057a7efbe0ef337010b0`.
Inspected native 90-frame realtime captures: `dev-spider-wide` (DX12, zoom 2.2)
and `dev-strider-detail` (Vulkan, zoom 0.9). Their lightweight previews show the
whole support/leg pose. The closer Spider pass and wider Strider pass are saved
as supplementary evidence, not extra acceptance requirements.

## Release delivery

One release build completed in 636.86 s with unchanged compile inputs.
Binary SHA256:
`b0987a85c3ded84bb8da0f6a72b476ae920ebeda1a1369839fd26e85fd4519e4`.
Both 90-frame realtime release traversals passed on Vulkan; their previews
were inspected: `out/walking599/release-spider-turn` and
`out/walking599/release-strider-reverse`. No paused scene or single-unit timing
is offered as battle FPS evidence.

Fresh read-only shortcut inspection confirms the same Полигон launcher chain.
The actual `play-range.ps1` selector was exercised with only `Start-Process`
intercepted; it selected the exact release hash. The shortcut itself was not
rewritten. The same exe separately ran both native release traversals.
`out/walking599/delivery-checks.json` records 25/25 checks passed, including
source/binary identity, final test evidence, all four replay/trace/anchor/IK
checks, both release captures and launcher selection. No game is left running.
No CPU/FPS improvement is claimed from single-unit traces.

## Native environment

The initial Vulkan baseline capture returned process code 0 but logged
`DeviceLost` and produced no images; it is explicitly invalid in its manifest.
The DX12 baseline retry produced two inspected lightweight previews in
`out/walking599/before-turn-dx12`. These are unpaused traversal captures,
not battle FPS evidence.

Subsequent development Vulkan captures passed. The initial device failure is
preserved as an invalid attempt, rather than hidden by its zero process code.

This iteration does not implement support-source identity/Ready from 594–595,
moving debris support, gate congestion, walking during explosions, or battle
FPS acceptance. Existing uncommitted work and the user's queue are preserved.

## Independent follow-up

После принятого здесь `ef0ff656…` Codex продолжил `planted604`. На более новом
`a0b56c7d…` независимый movement — **56/58**: rail unreachable planted и прежняя
сингулярность `Sole::clearance`. Это не отменяет доказанные результаты данного отчёта,
но запрещает переносить их на новую ветку. Независимый разбор — [604](604-independent-contact-window-review-20261114.md).
