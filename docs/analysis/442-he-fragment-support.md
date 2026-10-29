# 442 — Fragment support after a live high-explosive shot

The laboratory live HE tool produces ordinary `sim-core` fragments. A single
ground shot in a 128³ scene produced 218 fragments; after 150 ticks they were
all resting. A diagnostic checked the fragment support boxes against actual
occupied cells and found six resting pieces whose only apparent support was an
empty part of another piece's conservative collision box. After that first
fix a second diagnostic found seven pieces resting on a support beneath an
empty corner of the rider's own conservative box.

`Settled` still uses its spatial index and world-space boxes as a broad phase.
For queried irregular multi-cell fragments it now retains a frozen compact
pose and mask and checks the occupied cells after a box hit. Downward contact
and resting support refine both fragments; other movement retains the simpler
candidate-side check. Cell boxes use
the same scale, centre, basis and 0.49-cell half-width as existing world
collision. Single-cell pieces and section chunks keep the former box path.
The check is bounded by the fragment's at most 64 occupied cells, short-circuits
on a hit, and only executes on indexed broad-phase candidates. Fragment shape,
material, mass, velocity, sleeping rules, snapshot format and world voxel
size are unchanged. Rotated cell AABBs are still conservative, so this is not
an exact oriented-polyhedron contact test.

On the same deterministic HE shot the diagnostics found zero pieces supported
only by empty portions of neighbouring or rider fragment boxes. Regression
tests also check both missing corners and occupied corners.
Targeted tests cover real thin-fragment supports, support removal, rotated
neighbours, and consistent multi-worker wake order. The architecture gate
passes. The existing `zero_room_eviction_preserves_settled_contacts_outside_the_original_sweep`
test fails with the same result on unmodified `HEAD` and on the change; it is
not treated as a passing regression check. The small debug HE test took about
0.46 seconds before and 0.48 seconds afterward on one run each; these are not
a representative benchmark or a 4K FPS measurement.
