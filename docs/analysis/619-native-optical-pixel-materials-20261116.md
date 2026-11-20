# 619 — procedural native optical/pixel materials



Verified and delivered together with620 after their shared visual/release gate.

Current acceptance and exact delivery state: [620](620-coordinated-ui-motion-20261116.md),

out/ui620/ledger.json. Earlier Polygon618 remains the baseline until that gate.



One native material implements four explicit finishes: pixel phosphor with

ordered raster, granular warm cells, stepped edges and selected amber rails;

data dot matrix; optical interference/lens marks tied to the hull aperture;

and a local CRT event insert. The main menu composition is retained. Its former

full-image moving band and scan lattice are replaced by a bounded local insert.

Battle controls have no sync/chromatic distortion; service modes retain quiet

light backgrounds. Native text, icons and geometry are not sampled or displaced

by the shader. Decorations render beneath native glyphs.



96 material assets maximum; only local computed bounds receive MaterialNode.

Hidden/oversized surfaces shed the material and keep their native controls.

CRT480x80 maximum; general1100x700 and480000 logical square px. No new runtime

dependency, scene-texture read or fullscreen postprocess. Rare eligible events

permit40ms sync and65ms split, limited2px/1.25px; local flash ends220ms. Idle

noise is stable spatial grain.620 intensity35 removes sync/split;0 removes

procedural detail without removing native information.



Pixel catalogue previews come from real transparent model renders, cached

once with quantised cool facets and amber edges. Category/tier glyphs use a

fixed4x4 native mark pool. They follow the semantic amber/cyan/lavender/red

system and the mode matrix in docs/ui-style-system.md.



Evidence: out/ui619 first reserved-WGSL-identifier frames were invalidated,

even though the process returned0. The revised helper rejects GPU shader

processing/parsing/validation errors. Subsequent619 development renders passed;

final qualification and release use the combined620 source and native receipts.

No screenshot or paused UI run constitutes battle FPS evidence.
