# World-anchored RTS controls — native visual QA, 2026-11-11

final result: passed

## Target and scope

The user supplied a family of optical RTS concepts and explicitly requested
clickable controls at the selected unit/building in the world. This implements
that interaction in the existing native voxel game. It is an adaptation of the
concepts, not a screenshot recreation of their terrain, models or fixed HUD.

Selected visual references, both supplied by the user and opened this run:

- `C:/Users/artyo/Downloads/Precision-framed RTS factory build interface.png`, 1672 × 941: thin frames, recipe imagery, prices, queue and segmented progress.
- `C:/Users/artyo/Downloads/bbc5d763-af62-4d78-a82f-510e83e2559a.png`, 1672 × 941: clean cyan selection corners, white command symbols and an object-linked label.

The clean variant supplies the primary blue/white treatment; amber marks
prices and active controls. Existing native models supply the thumbnails.
No generated combat scene, invented units or simulated resource readings
are substituted for the actual game.

## Iteration evidence

1. `out/spatial-550/factory-preview/0000.png`, debug 1920 × 1080 — accepted as the first working prototype. The panel is beside the factory, with real recipes and its two queued tanks. P2: the selection box extended above occupied geometry because it used the allocated volume. Fixed by caching occupied local bounds and projecting their current poses.
2. `out/spatial-550/factory-4k/0000.png`, intermediate release 3840 × 2160 — rejected as final. P1: an unscaled scroll clipping rectangle cut the title, commands and queue when the branch was scaled 2×. Removed this clipping path; the bounded layout fits via size adaptation.
3. `out/spatial-550/clip-fixed-preview/0000.png`, debug 3840 × 2160 — opened and accepted after the correction. Title, cargo/network status, command row, four recipes, queue, segmented progress and hint all appear within the panel.
4. Final release captures: `out/spatial-550/final-factory-4k/0000.png` (3840 × 2160), `final-factory-1080/0000.png` (1920 × 1080), `final-tank-1080/0000.png` (1920 × 1080), `final-commander-4k/0000.png` (3840 × 2160), `final-factory-720/0000.png` (1280 × 720) and `final-factory-540/0000.png` (960 × 540). All six exited 0 with empty stderr. Factory/commander panels, commands, prices, queue and selection were inspected; the native 4K panel was also inspected unscaled in `final-4k-panel-detail.png`.
5. Final shared-state polish: selected tiers and ammunition/auto modes keep their amber active state. An empty tier explains why no recipes appear. The catalogue bridge rejects a previous scene/actor while a scene switch is settling.

The initial debug integration test set normalized pointer coordinates but
left Bevy's separate `cursor_over` field false. The corrected test sets both;
this was a test fixture error, not evidence of a native click-through bug.

## Final visual comparison

`out/spatial-550/comparison-full.png` pairs the full supplied factory concept
with final native 1080p; each fits a 1260 × 710 slot with its aspect ratio
preserved. `comparison-focus.png` pairs the concept's icon/queue region
(540, 700, 1320, 935) with the implemented contextual panel
(1090, 250, 1600, 685). Both sheets were opened and inspected. This is a
comparison of styling and function after the requested move into the world;
the layout is deliberately different from a fixed bottom HUD.

The final panel has the thin frame, white pictograms, cyan world selection,
amber prices/active controls, actual model previews and segmented progress.
No P0/P1/P2 issue remains in the reviewed contextual-panel paths. This does
not certify the older global HUD or every game scenario.

## Interaction and implementation checks

- One reusable object panel, four screen corner marks, a leader line and a small world-space footprint highlight. No UI entity per world voxel or per unselected actor.
- Panel follows the selected actor/camera, tries the opposite side near a screen edge, stays inside margins and pins its position while hovered. Offscreen/dead/unselected actors do not leave a detached card.
- Ordinary panel actions and catalogue actions remain the single command path. Native `Button`/`Interaction` controls are reused; no parallel economy or production implementation.
- Existing thumbnails are cached; occupied geometry bounds are cached per template and scene. Per-frame geometry work consists of transforming/projecting corners of the selected actor's parts.
- F8 and the header button switch to the detailed legacy HUD. The world panel shows six queue thumbnails; the complete paged queue remains available in that HUD.
- Laboratory, placement/editor, first-person tools and main menu keep their existing interface paths. Global header and minimap styling are outside this iteration.
- Local generation/demand and carried/stored matter use the current published game state. Costs come from the shared recipes. The paused QA scene legitimately shows 0% progress.

## Functional evidence

- Three spatial tests passed in `out/spatial-550-tests.log`: screen-side placement, capability-specific commands, and clicking the actual constructed world recipe button to enqueue a tank at its selected factory while blocking map input.
- F8/menu isolation passed again against the final source in `out/spatial-550-toggle-final.log`; final placement and capability checks passed in `spatial-550-placement-final.log` and `spatial-550-commands-final.log`.
- Existing optical-menu regression: 2 passed, `out/spatial-550-menu-regression.log`; six existing launcher/room/pause/input flow checks also passed in `out/spatial-550-flow-tests.log`.
- Architecture: 12 passed, `out/spatial-550-architecture.log`.
- Documentation: 3 passed, `out/spatial-550-docs.log`.

These tests inject Bevy interaction state; they do not claim a native OS mouse
or keyboard accessibility audit. Native screenshots verify rendering. No
screen-reader compliance or full combat FPS claim is made.

## Remaining polish / deliberate limits

- P3: model thumbnails use the existing filled voxel renders rather than the concept's schematic wire illustrations.
- P3: the older global header/minimap remain visually denser than the concepts; the old header is crowded/clipped at 960 × 540. The new object panel fits that viewport, while the main target is 4K.
- P3: small secondary labels at 720p; the main target is full-window 4K.
- Long queues are summarized near the object and fully inspectable via F8.

## Final checklist

- [x] User reference images and current-run native prototype inspected.
- [x] Real recipe/queue controls and shared action paths connected.
- [x] Excess bounding volume and 4K clipping found and corrected.
- [x] Final release captures inspected at 4K, 1080p, 720p and 540p.
- [x] Full-view and focused source/implementation comparison inspected.
- [x] Final release executable verified.

## Final build and process limits

`target/release/blast.exe`: 88,196,096 bytes, 2026-11-11 01:54:53 local time.
The successful final build log is `out/spatial-550-release-build-final-guard.log`;
11 min 44 s includes waiting for the previous build lock. A superseded
compilation was stopped and is not counted as a successful final build.
The final capture batch ended normally; no game or compiler process remains.

Captures use a paused four-actor 1536 × 256 × 1536 fixture, hidden windows,
30 FPS, two logical CPUs, BelowNormal priority and one physics/nav worker.
They do not measure battle FPS. The slow debug factory initialization was
not used as a performance benchmark.
