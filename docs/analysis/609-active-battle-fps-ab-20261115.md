# 609 — Active battle FPS A/B, 2026-11-15

Status: measurements complete; qualification **failed**, candidate is **not accepted for delivery**.
The canonical Polygon release607 remains unchanged.

## Qualification and scope

Frozen candidate source: `0df6bdd30eef0e6ebf8655f717ef6a72a351ce9268a0378681d822fcd24519f9`.
`out/battle609/qualification13/test-index.json`: 12 nav + 2 modding + 87 SIM
passed, one SIM failed, one existing manual benchmark ignored. The failure is
`walking_blast_actual_incoming_sole_revalidates_lands_and_continues`: real
excavation and fresh landing occur, but continuation is only 0.252 m and the
order does not finish. Worker1/2 and restore equality pass. It is not accepted.

The four real overlapping-blast detour courses now pass their original arrival
limits for Commander, Spider, Strider and Walker04. Nearby settling changes no
longer repeatedly cancel a valid route under currently validated walking soles;
current source/coverage and full physical support admission still see the dirty
events. Changes beyond those soles, cold invalidation and unsupported actors
retain route cancellation. No per-voxel sweep or anchor relocation was added.

The broad suspension-height correction which made the first 5/5 blast run pass
was rejected: it regressed flat/hill Strider height and the deep-pit course.
The original half-body-voxel correction is restored; all three regressions pass.
A trial reach check during source revalidation did not fix continuation and was
also reverted. No assertions, deadlines or IK tolerances were weakened.
The continuation observer now retains the first actual landing, instead of
resetting its origin at every subsequent step.

Dev candidate built successfully in 61.614 s from this unchanged source,
SHA `0afad9a1c1871a79c756ec547e35d8f49f99156650169713f4ff03ae76435856`.
Two native Vulkan continuations produced four inspected companion JPEGs
(`visual-review.json`). They are one-unit geometry/UI diagnostics, not battle FPS.

## Measurement method

Primary comparison: delivered release607 `d7f0f684…` (UI602 and mechanics607)
versus the provisional609 release candidate, on identical current external inputs.
The preserved pre-UI596 binary `79fffee8…` is an additional historical point;
its original source/build receipt is unavailable, so it cannot establish an
isolated UI cost. Hashes and original use are in `before-ui-provenance.json`.

Each primary version runs twice for 30 s at 1920×1080 and 3840×2160, in ABBA
order at each resolution. `out/battle609/profile-direct.py` uses the same native
arguments as `scripts/profile-range.ps1` for `--world-battle`
with AI versus AI, stock tuning, Vulkan, FPS ceiling200, the same eight logical
CPUs and two workers in each compute/physics/navigation/Rayon pool. Native
windows stay in the background. The previous two-CPU10 s pilot is excluded.
The first canonical PowerShell capture completed but its wrapper did not return;
that capture is also excluded. The replacement launches the exe directly, owns
its process handle and samples Windows PROCESS_MEMORY_COUNTERS_EX at 1 Hz.
It has a 180 s cap and can terminate only its own child handle. See
`profiler-wrapper-interruption.json`; no queue/app settings were changed.
No pause, screenshot capture, one-unit fixture or old128→126 UI figure is used.

The map is `data/range-km.ron`, 10000×512×10000 world units (1×1 km);
`battle_world` extends its vertical bound to 1024 for the construction yard.
The authored start has 64 objects:62 battle placements plus2 yard structures.
Economy production adds actors during combat (the excluded pilot ended with73);
this includes buildings and armed emplacements, not only mobile soldiers.
Final roster/alive counts,
shots, damage, removed voxels, active waves, fragments, movement orders,
material jobs and simulation debt are recorded separately for every run.

Frame intervals are consecutive `frame_start_ns` differences, checked against
`observed_frame_ms`, including stalls. FPS is 1000/mean interval. p50/p95/p99 are
frame-time percentiles in ms, floor(N*p) order statistics. Whole-run results and
a shared simulation-tick window are both retained; slower simulation does not
qualify as a performance gain. The camera preset and inputs match, but movement
changes can change scene contents; this is not pixel-identical replay footage.

Private bytes and working set are sampled at 1 Hz and restricted to the actual
profile epoch; reported maxima are sampled maxima, not an exact OS peak or VRAM.
Background diagnostic profiles can be valid comparisons while remaining
ineligible for the profiler's foreground FPS-budget certification. That flag is
preserved. Source/binary stability and all data hashes are checked for every run.



## Results — primary release A/B

Windows 11 build 26200, AMD Ryzen 9 9950X3D; logical CPU IDs 0–7, BelowNormal.
All eight native logs identify NVIDIA GeForce RTX 5090 / Vulkan / DiscreteGpu.
The process affinity and worker limits are part of this controlled comparison;
these figures are not an unrestricted-all-cores hardware maximum.
Both binaries contain the exact current battle, yard and economy RON text.
A 96-file audit finds 79 shared exact current RON literals and zero presence
mismatches between607 and609. All 97 external RON hashes, switches, adapter,
physical resolution, camera controls and source/binary stability checks pass.

Each row pools both repetitions' consecutive frame intervals. Every capture is
30 s; startup and world loading precede its measured profile epoch.
Percentiles are frame times, not FPS percentiles.

| Release | Physical resolution | Full-window FPS | p50 ms | p95 ms | p99 ms |
|---|---:|---:|---:|---:|---:|
| 607 before | 1920×1080 | 22.71 | 38.82 | 107.48 | 129.56 |
| 609 candidate | 1920×1080 | 22.49 | 36.98 | 112.42 | 147.07 |
| 607 before | 3840×2160 | 21.43 | 41.76 | 108.57 | 132.20 |
| 609 candidate | 3840×2160 | 20.96 | 39.82 | 114.05 | 149.72 |

A slower simulation executes less combat in the same wall time. The shared
simulation interval is ticks 40–411, inclusive, for every primary run. This
removes the first two simulation seconds and ends at the shortest run's last
tick. It includes the heavier settled battle rather than comparing different
lengths of simulation. It does not force identical outcomes after a mechanics
change; workload differences are shown below.

| Release | Resolution | Shared-tick FPS | Range across repetitions | p50 / p95 / p99 ms |
|---|---:|---:|---:|---:|
| before | 1920×1080 | 17.71 | 17.63–17.80 | 57.24 / 115.60 / 137.31 |
| after | 1920×1080 | 17.17 | 17.10–17.23 | 61.89 / 120.01 / 151.00 |
| before | 3840×2160 | 17.36 | 17.34–17.38 | 57.59 / 115.70 / 136.31 |
| after | 3840×2160 | 16.54 | 16.50–16.58 | 63.77 / 121.19 / 152.22 |

At 1920×1080, candidate shared-tick FPS changes by -3.08%.
At 3840×2160, candidate shared-tick FPS changes by -4.72%.
These runs show a regression, not a performance improvement. They are two
repetitions per version/resolution on one bounded machine, not a universal
hardware claim. No foreground budget certification is claimed.

## Memory — process, separate from FPS

1 Hz Windows PrivateUsage / WorkingSetSize samples inside each actual profile
epoch. GiB=2^30 bytes. Maxima below are the worse sampled value of two runs;
mean WS pools the samples. These are not exact instantaneous peaks or GPU VRAM.
Per-run p50/p95/p99, maxima and sample counts remain in `battle-comparison.json`.

| Release | Resolution | Mean WS GiB | Max WS GiB | Max private GiB | Samples |
|---|---:|---:|---:|---:|---:|
| before | 1920×1080 | 15.60 | 15.83 | 18.86 | 61 |
| after | 1920×1080 | 15.60 | 15.79 | 18.82 | 60 |
| before | 3840×2160 | 15.68 | 15.86 | 19.15 | 60 |
| after | 3840×2160 | 15.65 | 15.82 | 19.11 | 60 |

## Roster and world workload

Authored start: 64 objects, including structures; economy grows the roster to 73.
The end roster includes destroyed actors: it is not the alive count. Alive
counts below are allied/enemy. The battle remains Fighting, with actual shots,
damage and excavation in every run. `moving_units` means actors with movement
orders, not measured nonzero velocity; it peaks at 34. Up to 4 live blast waves
overlap. Fragment records include settled debris and are not a count of active
rigid bodies. This preset is an actual combat/destruction workload; material
jobs and corpse records in the sampled frame counters remain zero, so it is not
a mining-economy or 5000-unit stress result.

| Run | Roster / alive | Firing actors | Shots | Native damage counter | Voxels removed | Max fragment records | Sim / real time |
|---|---:|---:|---:|---:|---:|---:|---:|
| direct-before-1080-r1 | 73 / 39+25 | 21 | 1270 | 180130 | 756909 | 76822 | 0.7259 |
| direct-after-1080-r1 | 73 / 40+26 | 20 | 1056 | 184650 | 800461 | 71775 | 0.6863 |
| direct-after-1080-r2 | 73 / 40+26 | 20 | 1049 | 184490 | 800185 | 71392 | 0.6835 |
| direct-before-1080-r2 | 73 / 39+25 | 21 | 1269 | 180108 | 751915 | 76615 | 0.7237 |
| direct-before-4k-r1 | 73 / 39+25 | 21 | 1264 | 180090 | 751589 | 76274 | 0.7207 |
| direct-after-4k-r1 | 73 / 40+26 | 20 | 1044 | 184388 | 800022 | 71141 | 0.6812 |
| direct-after-4k-r2 | 73 / 40+26 | 20 | 1049 | 184490 | 800185 | 71392 | 0.6816 |
| direct-before-4k-r2 | 73 / 39+25 | 21 | 1267 | 180108 | 751674 | 76374 | 0.7232 |

All runs fail the real-time simulation criterion: candidate ratios are 0.681–0.686,
versus 0.721–0.726 before. Frame counters reach 19 ticks of scheduling debt and
report discarded real-time tick budget. This is not a paused scene, and the
render rate does not certify a real-time battle. Raw per-frame counters for
debt, dropped budget, waves, navigation, fragments and material work are kept in
each CSV; full-run and shared-tick summaries remain separate.

## Historical pre-UI596 point — not the controlled primary pair

One 30 s run per resolution, same authored battle and external input files.
The original build/source receipt is missing and embedded walking-terrain RON
differs from the current version. It combines old mechanics/configuration and
UI; it cannot measure isolated UI cost or replace the repeated607/609 pair.

| Resolution | Full-window FPS | p50 / p95 / p99 ms | Max WS / private GiB | Sim ratio |
|---|---:|---:|---:|---:|
| 1920×1080 | 28.34 | 28.83 / 83.45 / 112.90 | 15.98 / 18.98 | 0.8090 |
| 3840×2160 | 26.27 | 31.82 / 86.95 / 115.23 | 16.00 / 19.29 | 0.8011 |

## Build, validation and delivery

The single candidate release build passed in 638.952 s, with unchanged source;
SHA `9a54c0df47780feb3427f9b9a25d1ebfa2d300e8b3fdccaa6b59f840e604a12f`.
It is `target/release/blast-candidate.exe`, a gated opt-in Cargo bin using the
same entrypoint. Default `blast` remains the ordinary target. The alias feature
has no runtime code branches. No dependency or balance-data change was made.

Focused qualification:101 Rust checks passed, 1 failed,1 old manual benchmark
ignored. Additional architecture/docs checks passed 12+3; Python runner/profile/
audit checks passed 15+139+9. Four dev JPEG previews were inspected. All ten direct
native battle captures completed with exit0 and stable source/binary/inputs.
This does not turn the failed underfoot check into a pass.

**Poligon was not updated.** Its original shortcut is unchanged; a fresh read
and interception of the actual launcher's selector after the candidate appeared
still identify release607 `d7f0f684…`. The same607 binary actually ran the four
before captures. Candidate is held because qualification and real-time combat
performance fail. No app database, queue, context setting or archive was changed.

Done: real battle A/B in release1080p/4K; raw clocks, workload, memory, build and
launcher evidence; scoped cancellation fix and strict regression checks.
Remaining: physically valid underfoot continuation and combat performance that
keeps simulation in real time. The candidate is not declared ready or delivered.

Evidence under `out/battle609`: `battle-comparison.json`, `battle-groups.json`,
`historical-preUI596.json`, `qualification13/test-index.json`,
`architecture-docs/test-index.json`, `python-checks.json`, `build-dev.json`,
`build-release.json`, `visual-review.json`, `primary-embedded-input-audit.json`,
`launcher.json`, `shortcut.json`, `profiler-wrapper-interruption.json` and the ten
`direct-*` folders. Every direct folder includes the exact command, binary/data
hashes, process exit, frame CSV, native log, epoch-aligned memory samples and
validity. The abandoned PowerShell capture and two-CPU pilot remain diagnostic
artifacts and are excluded from the primary statistics.

Final evidence-integrity audit:56/56, `out/battle609/final-audit.json`.
Game acceptance remains false; this audit verifies the evidence, not readiness.
