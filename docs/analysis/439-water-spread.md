# Faster water spreading

Free fall retains the existing fixed-point gravity integration. After landing,
horizontal flow equalises up to half of a neighbouring volume difference per
visit instead of a quarter; stock viscosity is reduced from 200 to 40 per
thousand. The neighbour order rotates deterministically, so repeated visits do
not systematically favour the same direction. Volumes remain conserved and
the existing transfer and check limits are unchanged.

In a 4 × 4 × 8 cell column on a flat surface, after 20 simulation ticks the
most distant floor cell containing at least one eighth of a cell was 7 cells
from the reference point before and 17 cells after this change. This is a
specific sparse-front diagnostic, not a measured flow velocity or a claim of
physical accuracy. Both runs hit the 1024-transfer budget. Very large bodies of
water can still fall behind their desired update rate; the renderer also
limits chunk updates. Horizontal flow remains an approximation without a full
momentum or shallow-water solver, and can spread into very thin layers.

Focused headless tests cover spreading in all directions, exact volume,
gravity, walls and openings, save/replay, wetting and laser interaction. FPS
and a native visual run were not measured for this change.
