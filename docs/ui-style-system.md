# Perimeter UI — shared signal system

The interface uses one semantic system with distinct rendering registers.
Amber identifies selection, production and progress on a quiet navy HUD base.
Cyan identifies spatial projection and its optical controls.
Lavender is a secondary accent. Red identifies threat or destructive action.
Meaning is also conveyed by labels, shapes and state, never colour alone.

| Mode | Main register | Application |
|---|---|---|
| Battle HUD | Pixel | Navy islands, square glyphs, stepped corners, amber focus and segmented readouts |
| Static categories/catalogue | Pixel | Crisp silhouette previews and restrained amber selection |
| Production/progress | Pixel + dot-matrix | Amber segments, raster telemetry and real model previews |
| World object controls | Optical | Cyan emitters/rays/depth and selected controls; local amber work |
| Main menu | CRT + optical | Existing composition, amber focus, quiet scan texture |
| Battle room | Pixel + optical | Modular preparation grid, explicit red opponent |
| Laboratory/editor | White/cyan service | Quiet light surfaces, thin frames, amber active tool |
| Settings/help | White/cyan service | Clear text and restrained frames |
| Numeric data/previews | Dot-matrix | Localised dot glyphs and raster model images |
| Transitions | CRT | Brief synchronisation/reveal; readable text remains steady |

Grid base:4 logical px. Primary spacing:8/16/24. Optical line:1px;
pixel line:2px. Corner extent:8px. Common text ladder:12/14/16/20/28/40px.
Small captions retain a readable narrow face; pixel and data faces are distinct
variations of embedded Handjet, optical/service text uses embedded Oswald.
Fonts and icons are embedded; no runtime font downloads are required.

Interaction times: press90ms, hover90ms, reveal240ms, page transition240ms.
Animation changes decoration and opacity; it does not move hit targets.
Pixel chrome stays still during combat. CRT texture belongs to menus and
transitions, not the world or laboratory measurements. Decorative nodes pass
pointer input through. Threat, focus and production remain legible without
animated glow. Existing commands, quotes and input blocking retain their rules.

The main menu keeps its baked scenery, composition, hierarchy and native
controls. Other modes share tokens and semantics rather than its background.


## Spatial hierarchy

Dynamic readouts occupy the upper, nearer layer. Production has one global
upper island, independent of selection or an open catalogue. It shows current
orders, real progress and following items. Multiple allied producers have a
compact aggregate with paged detail disclosure. The queue is not repeated in
the lower dock or in a world projection.

The minimap is anchored at the lower left. Categories, navigation and permanent
orders keep their lower anchors when selection changes. Navigation, categories
and commands occupy separate islands with click-through gaps. Debug, editor
and mode tools are disclosed by the Systems overflow, outside the primary row.
The normal factory palette uses compact independent islands. Desktop object
projections additionally offer a native optional recipe shelf; opening it hides
the normal recipe row while keeping category and command anchors stable.
Compact viewports use one lower recipe drawer rather than a second projected
recipe layer. Details and auxiliary actions occupy separate islands.

HUD scale follows the smaller viewport ratio to1920×1080, bounded1–2.
960×540 keeps native-size text;3840×2160 uses2×.
Native menu text has a12 physical pixel floor, independent of the fitted
menu composition; large headings retain their authored size. Shared entry
controls restore their authored font size when returning to a session. Short notices occupy the
upper gap between the measured resource island and the production island,
with bounded height and scrolling. They do not cover the object projection.

Laboratory readouts and transport state are above the scene. Static tools,
library and auxiliary actions are below. Transparent layout wrappers do not
block world input. Only visible island rectangles and actual controls own hits;
empty space between them remains interactive world space.

Below1400 logical pixels in width or780 in height, the laboratory uses a
compact dock layout. Text keeps its native size. Library categories, tools,
scene composition and auxiliary actions scroll inside bounded islands;
inspector tabs have a separate bounded scrolling area above the data viewport.
Overflow clips at island boundaries. The world remains visible between docks.

Producer detail clicks bind to the actor and scene generation actually shown
in the row. Queue completion or scene replacement does not retarget a click.
Compact world details remain above the lower dock; recipe details use a separate
narrow island above the minimap and left of catalogue cards.

Below780 logical pixels in height, the recipe catalogue uses a100px island,
68px cards and32px model previews. Captions keep their native14px size.
Catalogue, minimap and permanent command anchors are independent of selection.
Projection placement reserves the actual viewport-specific catalogue height.
Recipe quotes anchor above the fixed minimap with an8px gap. Only a visible quote
participates in projector occlusion; a hidden quote never hides commands.

Recipe details have a single live-quote implementation for normal hover and
capture previews. The narrow detail island keeps mass, energy work, demand
and generation; only field labels are abbreviated.


## Spatial projector lifecycle

A selected hull supplies an occupied roof emitter. Six perspective guides
converge to that emitter; three wire planes, depth struts and luminous vertices
form one bounded rig. Only rear decoration has slight parallax. Ink and click
targets remain on the final stable plane. A local elliptical source response
and travelling guide pulses carry the selection signal.

Selection opens the aperture from the emitter in320ms and then settles to quiet
light for reading. Fold, deselection and screen-mode exit return the rig to the
source in220ms. A folded projection retains a compact restore control. Changing
actor closes the previous rig before opening the next. Scene replacement clears
the obsolete rig. Projection glass uses the actual source-relative aperture;
the menu CRT wipe is separate.

Opaque controls avoid the selected hull, the central aim region, the upper
production island and the lower dock. If no safe rectangle fits, only the
source signal remains until space is available. Hovered controls keep their
position while that rectangle remains safe. Decorations always pass input.


## Procedural native finishes

Pixel phosphor combines ordered raster, stepped contour, sparse cyan cells and
selected rails. Data surfaces use a quieter granular dot matrix. Optical glass
uses fine interference, lens ticks and the existing source-relative aperture.
CRT occupies a local event insert, without a persistent filter over artwork,
text or the battlefield. Native backgrounds retain their real state colours.

Procedural decoration is drawn below native text. No glyph, icon, screen or
scene texture is sampled by the surface shader. Text geometry and font state
are unchanged. Model previews are cached from actual geometry with clear
silhouettes, unaltered facets, nearest sampling and a transparent canvas.
Dot-matrix and warm quantisation belong to local data/CRT views. Category glyphs
use a fixed four-by-four native pool. Amber accents production, not whole panels.

The material pool has96 entries. A finish draws only after its computed bounds
pass the local size/area limit; oversized or hidden surfaces lose MaterialNode
and retain native controls. CRT inserts are at most480×80 logical px. General
finishes are at most1100×700 and480000 square logical px. Layout-only wrappers
never receive finishes. Quiet service modes retain their light native surfaces.

Sync is allowed on one of three eligible optical/CRT events for40ms. Decorative
chromatic split is at most1.25px and only during a65ms event window. Both become
exactly zero after the event; pixel combat controls have no sync/split. Local
flash ends within220ms. Idle noise is spatial, fine and stable; temporal noise
belongs only to the local event. Menus retain their composition without the
old full-image scan lattice or moving band.


## Event-driven UI motion

Native controls and text occupy their final layout immediately. Only decoration
and a bounded20% background-opacity ease carry entry; no input waits for motion.
Hover/unhover lasts90/60ms, selection120ms, panel entry240ms and exit180ms,
disclosure180ms, queue advancement200ms and real completion280ms. Tooltip
intent delay is120ms. Pixel frame segments assemble from the invoking control;
optical projections retain their hull emitter and320/220ms aperture lifecycle.
Tab underlines transfer from the previous selection to the next. Closing strokes
return toward the source. Light service surfaces use quiet cyan and dark amber.

Progress bars interpolate toward published progress for80ms, never beyond it.
Numbers remain live. A new saved production-exit marker triggers completion;
cancellation or a changed queue head triggers advancement. Progress ticks alone
do not trigger repeated flashes. Scene replacement clears obsolete events.
Projection guide pulses repeat only for actual production, while rear parallax
responds to camera position rather than an idle breathing loop.

There are at most512 visible or closing tracked controls,16 simultaneous effects and192 native
stroke pieces. All decorative pieces pass input. Despawned controls leave the
registry. Effects settle without idle timers spawning new events. The settings
control UI effects cycles100/35/0 percent and persists locally in out/ui-motion.txt.
Reduced35 disables sync/chromatic accents and dims motion;0 removes decorative
strokes and material detail while keeping native text, state colours, geometry,
progress and commands. CLI --ui-effects is a non-persistent capture override.


Tooltip islands sit outside scroll/clipping containers while retaining their
world/control source and UI scale. In-flight opening/hover frames follow the
current measured layout; closing frames retain their last visible rectangle.
A newly published model-preview texture has one transparent extraction frame
instead of a white fallback quad. This never gates text or commands.


Projection placement also excludes the projected bounds of other live actors,
in addition to the selected hull, aim centre and fixed HUD islands. A pinned
panel is revalidated as the scene moves. If no safe rectangle exists, the
emitter signal remains until there is room for the controls.
At0 effects, pixel/data/CRT material nodes are absent; optical glass uses a
flat material path. Native information and input geometry remain available.
