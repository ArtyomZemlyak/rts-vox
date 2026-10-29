# 443 — Molten material drifting across flat ground

`Heat::step_molten` moved any supported molten cell sideways into empty air,
even when it was a single isolated cell on a level surface. The repeated
coordinate/tick choice made the cell wander until its heat dropped below its
freeze threshold, producing scattered solidified drops far from the source.

Downward motion remains the first choice. When direct descent is blocked,
the solver now checks all horizontal neighbours for a downward diagonal first.
A move across level ground is allowed only when molten material immediately
above the source supplies pressure. A lone supported drop therefore stays
in place, while a stack can spread and material can still run off a ledge.
This is a bounded cellular approximation, not a full momentum or viscosity
solver. No extra per-cell state or snapshot field is introduced; the existing
work and move budgets remain in force. Empty diagonal cells are checked before
their same-height neighbours to avoid unnecessary checks on flat ground.

Focused tests verify steel, concrete and glass drops on a floor cool in place;
a hot drop descends a step and settles; a two-cell stack spreads while
conserving both cells; dense melt respects the existing move budget; and a
laser-heated steel fixture continues flowing and reproduces after restore.
Architecture tests pass. No representative 4K FPS measurement was performed.
