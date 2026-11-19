# 610 — Walking support recovery and combat qualification, 2026-11-15

Status: complete for the current walking-support recovery item. Mechanics104/104, architecture/docs15/15, current native dev, eight final release A/B runs and launcher delivery verified.
First unfinished item: actual incoming-sole Strider continuation after explosions.
Final frozen source: `db0e5cde2939f1de3f18ba1d48065d781ef7b1724640f908d4e88ea7c328e460`.
The qualified release replaces delivered607; the previous binary remains preserved in `out/battle609/before-release.exe`.

## Physical correction

608/609 left a real failure: Strider earned a new Ready landing after the actual
incoming-sole impact but moved only0.252m afterwards. Bounded motor/anchor traces
separated a refused `current_pose` from terrain collision, lost legs or engine
failure. The current contact points were reachable; the next spring proposal was
not admitted.

Ordinary suspension fitting runs first. Only after it fails, a quadruped can
retain a nearby previous chassis height inside the existing half-body-voxel step
bound and configured reaction travel. Admission, lookahead, parked settling and
execution receive the actual previous chassis. Every contact stays at its world
anchor; no horizontal lane-fit transfer is introduced.

Foot intent and cadence are separate from actual body yaw. Lane checks use the
executed yaw, including a late contact alternative. A stationary automatic
quadruped route with a rejected landing can try one straight astern preparation
step through the installed actuator limits and full Ready support proof, followed
by ordinary terrain/collision admission. It uses existing escape speed, damage
response and terrain resistance. The route/goal stays intact; normal nonzero
motion takes priority next tick. Manual pilot control has no such fallback.

The strict incoming-sole course now reaches its original deadline after93
landings and approximately35.08m of continued travel. Actual Ready IK error and
planted XZ sliding are zero. Its largest supported vertical root step is0.1603m
at course tick35 with no live waves at that instant; this is not called zero body
motion. All original flat/hill/pit arrival and jump assertions remain.

## Support-query cost

The mechanically successful R7 was rejected after actual release battle FPS
regressed substantially. R10 exact Drive/Move/intent proof reuse alone did not
resolve it. General query caches and a sample-only cache also failed the CPU
cost gate and were removed. They do not appear in the final runtime code.

Final runtime optimizations preserve the proof itself:

- identical saturated motor candidates reuse an exact fit result inside one
  immutable motor selection; the original ranking is unchanged; once a command is completely proved,
  inferior ranks are pruned. Without a feasible command the refusal search is
  complete. Diagnostics count executed proofs;
- identical normalized tilt and height trials are skipped, retaining the first
  original tie and its angular damping;
- point queries keep cheap coarse ownership and build direct column rows only in
  queried busy pages; sparse/oversized cases stay bounded and catalogue-ordered;
- stable rays that cannot replace the current proof, and rays whose conservative
  rounded start cannot affect floor/contact evidence, are skipped. Negative or
  invalid upper bounds retain the full ray path.

No support result is reused across ticks, source events or units. Source IDs,
content/pose revisions, Moving/Unsupported, stable-only debris and the separate
post-landing Ready transition retain their existing rules. There is no new
per-voxel world scan or delayed support refresh.


The prior0c release `780b69fc...` was also held: eight valid native runs restored
whole-period FPS near baseline, but the common40..418 active-tick window fell
17.45 to16.67FPS1080p and17.01 to15.74FPS4K; p99 rose approximately136 to182ms.
Those runs remain in `final-perf/`. They are not the final candidate's results.
A per-Probe bucket-cache experiment did not improve total cost and was removed.
The retained lazy source rows and rank pruning are measured in `playable19-cpu/`;
actual final native runs use a new `native19-perf/` directory.

## CPU diagnostic parity

The old authored `--world` benchmark has62 objects and remains available. The new
`--playable-world` diagnostic mirrors native setup: editor subsoil, adjusted
battle placement, build yard, live economy, default weapon configuration and
64 initial objects. It grows to73 records. CSV unit counters grow with production;
new plan labels are classified and other observer intervals stay measured.

On299 ticks, its authoritative hash exactly matches native R10 release
`9225945516040e33`. With2 physics/navigation/Rayon workers, optimized opt1 and
full-world debug assertions disabled, movement mean fell107.773 to41.664ms/tick;
Strider5 pose cost fell72.293 to6.476ms/tick. Total process time fell76.34 to53.78s
and tick p99 fell1597.832 to173.534ms. Both endpoints keep the same native battle
hash. These CPU intervals are not FPS. Final native release A/B is recorded below.

Shallow62-object and64-object diagnostic trials were not native-equivalent;
they are excluded from the native combat comparison. Earlier rejected binaries,
failed tooling invocations and measurements remain under `out/walking610`.

## Validation and delivery

Final focused4/4 pass: dense4096-piece ray/order boundaries, buried/overhead/live
carrier evidence, actual incoming sole and real deep-pit detour. All four strict
underfoot checkpoint binaries match mechanically accepted R7 byte for byte.
Full qualification104/104 passed (nav14, modding2, SIM88; failed0, ignored0).
All24 checkpoint binaries match R7. The final dev build and one current90-frame
native continuation passed visual review through its companion JPEG. Previous
Spider views are retained alongside physical checkpoint equivalence. Final release
A/B and architecture/docs15/15 passed; the actual launcher selector receipt matches the delivered release.

Native battle A/B uses actual consecutive frame-start intervals,30s active AI
combat, stock inputs, Vulkan,200FPS ceiling and2 compute/physics/navigation
workers on the same8 logical CPUs. No paused screenshot or interface-only FPS
is combat evidence. Memory is sampled Windows working set/private commit inside
the profile epoch, not VRAM or a subsecond peak guarantee.

The stock initial workload is36 mobile units and28 structures on the kilometre
map (62 battle objects plus storage/factory yard). Production, survivors, shots,
damage, removed voxels, fragments, live waves and pending work are reported with
the actual final native runs. UI style remains602; this iteration changes support
mechanics and measurement, not the optical brief.

Evidence root: `out/walking610`; focused `index19/test-index.json`, qualification
`qualification19/test-index.json`, `index19-final-captures`, `playable14-cpu`,
`playable19-cpu`, `ledger.json`, final native/build/performance/launcher receipts.

No balance data, dependency, save schema, queue mode, context limit or Codex app
database was changed. The archived chat was not started, deleted or unarchived.

## Final native release comparison

Source `db0e5cde2939f1de3f18ba1d48065d781ef7b1724640f908d4e88ea7c328e460`, release `a0bc610e810fc0b4f1609d90e1fe68103a20a5247d992bb5adb7076c595690dd`. Baseline is delivered607 `d7f0f684...`. Each resolution has two runs per version, ordered before/after/after/before. Shared active window: ticks40..430.

| Version | Resolution | Whole30s FPS | Active-window FPS | Active p50/p95/p99 ms | WS/private sampled max GiB |
|---|---|---:|---:|---|---|
| before | 1920x1080 | 21.77 | 16.49 | 60.02/115.51/138.37 | 15.83/18.86 |
| after | 1920x1080 | 22.74 | 17.36 | 62.45/108.92/122.44 | 15.83/18.63 |
| before | 3840x2160 | 20.68 | 16.51 | 60.02/116.25/137.75 | 15.87/19.14 |
| after | 3840x2160 | 20.36 | 16.12 | 64.97/110.64/122.95 | 15.87/19.20 |

Whole-period percentiles:

| Version | Resolution | p50/p95/p99 ms |
|---|---|---|
| before | 1920x1080 | 42.14/109.64/129.65 |
| after | 1920x1080 | 39.50/105.78/118.21 |
| before | 3840x2160 | 45.78/111.80/129.90 |
| after | 3840x2160 | 46.83/108.06/119.36 |

Measured workload (whole-period maxima/totals; records include structures and casualties):

| Version | Resolution | End records / alive | Removed voxels | Shots | Peak fragment records / live waves | Sim/wall rate |
|---|---|---|---|---|---|---|
| before | 1920x1080 | [73, 73] / [[39, 25], [39, 25]] | [750994, 751589] | [1255, 1264] | 76274 / 4 | 0.715..0.721 |
| after | 1920x1080 | [73, 73] / [[39, 26], [39, 26]] | [845031, 845031] | [864, 864] | 81451 / 4 | 0.724..0.725 |
| before | 3840x2160 | [73, 73] / [[39, 25], [39, 25]] | [751278, 751278] | [1260, 1260] | 75939 / 4 | 0.718..0.718 |
| after | 3840x2160 | [73, 73] / [[39, 26], [39, 26]] | [842924, 835555] | [860, 852] | 81203 / 4 | 0.717..0.720 |

The workstation is Ryzen9 9950X3D / RTX5090. Actual child affinity0xff and BelowNormal priority were verified, alongside2 compute/physics/navigation/Rayon workers, Vulkan, stock inputs and200FPS ceiling. These are controlled active background diagnostics. The manifest keeps AcceptanceEligible=false for unrestricted foreground performance; real-time and project FPS targets are assessed separately from functional delivery. No paused or one-unit capture contributes to this table. Memory is Windows working set/private commit at1Hz within the profile epoch; VRAM is not measured.

`moving_units` counts destinations, not measured chassis velocity. Shots, damage, positive voxel removal and live overlapping waves establish actual battle work. Per-frame counters and both raw repetitions remain in `native19-perf/`. Changing mechanics changes the resulting battle workload; identical starting inputs do not imply identical destruction totals.

## Acceptance receipt

Focused4/4 and qualification104/104 (14nav +2modding +88SIM) passed without ignored tests. All24 checkpoint binaries are byte-identical to the mechanically accepted R7. Final dev19 and its90-frame realtime continuation passed companion-JPEG review. Architecture/docs15/15 passed. `audit19.json` binds the frozen source,104 mechanics checks,24 checkpoint hashes, the current native JPEG, eight native battle runs and the final release. `delivery19.json` records the atomic canonical replacement; `launcher.json` verifies the actual launcher selects `target/release/blast.exe` with SHA `a0bc610e...`. The desktop shortcut hash is unchanged. Selector verification intercepts process start; the same binary was actually executed in all eight native A/B runs.

Relative performance assessment: common-window FPS improves5.3% at1080p and regresses2.4% at4K; p99 improves11.5%/10.7%. This is measured functional delivery with a disclosed small4K mean regression, not an absolute performance qualification. Sim/wall remains approximately0.72; real-time and project battle-FPS targets are unmet. No further optimization or another queue item was started as part of finalization.


## Current-tree reacceptance622, 2026-11-16

The R10/102-check failure is historical. A fresh current-tree106-check and eight-run active battle ABBA passes strict no-FPS/no-SIM-regression against preserved607 at1080p and4K. Binary c644e0b4... is unchanged. See [report622](622-walking610-current-combat-reacceptance-20261116.md) and out/walking622/acceptance.json. The small4K regression disclosed above is historical; it is not reused as the new gate. Absolute20Hz remains unqualified.
