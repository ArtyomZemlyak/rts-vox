# Laboratory section metrics: first stage

The section panel can display heat, moisture, phase, material strength and
density, falling-grain speed, deformation work, burning time, water fill and
material identity. It samples the existing
10 cm world grid on a 48 × 48 view-facing patch centred on the selected cell.
Three extents of 4.8, 9.6 and 19.2 m use 10, 20 and 40 cm sampling steps.
With the section disabled it casts bounded rays and colours exposed voxel
faces. The overlay is read-only and uses a single mesh. Surface construction
is divided into twelve batches of 8 supersampled rows on successive frames; each sample
observes the state on the frame on which it is collected. A subsequent pass
refreshes it after camera or simulation changes. Candidate unit bounds are
filtered once per batch, and exposed neighbouring faces fill sampling gaps.
The ordinary voxel inspector can skip granular motion
lookup when it is not needed for a colour map.

Heat is simulation energy, not temperature in degrees. Water has no stored
temperature; its heat is marked unavailable. The speed of a falling water
column is available; lateral water speed is not tracked. Unit heat and unit
deformation use their body-cell data. Unit strength, density and moisture are
marked unavailable rather than substituted from the ground. The
speed layer covers free-fall water, granular free fall and unit drive speed,
but not unit rotation or shove. Sleeping ground has zero speed.
The perspective live-tool view uses the current tool target or its forward
point. Up to 64 nearby debris fragments have separate overlays with their
actual shape and presentation pose. Work is capped at 512 candidates scanned
and four new meshes per frame. Their material and velocity are read from the
fragment, while untracked heat, moisture and deformation are marked unknown.
Detached voxel bodies are not yet included. Phase uses the
same material classifications as the inspector. Colour scales are fixed and
shown in the panel; out-of-range values saturate.

This is not the requested complete scene-wide visualisation. The section patch
samples one camera-facing plane, so its oblique samples are not an exact
voxel-face mesh. Only the selected local window is covered, and larger
windows sample sparsely. Only 64 fragments are displayed simultaneously and
detached bodies are not included. A full-scale
performance measurement has not been performed. Hidden 4K visual checks
verified the section depth fix and improved surface coverage after neighbour
completion. Both used a paused steel slab and a 30 FPS limit, not a battle FPS
benchmark. Hidden 4K fixtures also checked the perspective view, displayed
legend and a single steel fragment coloured by speed; the latter logged its
overlay creation. These paused captures do not verify live movement or FPS.
A debug-only whole-patch CPU check took about 173 ms on the test
fixture; production splits this work across twelve frames, but that number is
not a release-build or FPS measurement.
