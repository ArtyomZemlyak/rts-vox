# Recent laboratory systems review

Targeted, low-priority CPU tests cover body landing and restore, water flow and
conservation, buoyancy, granular free fall and saved motion, replay recording
and seeking, metric sampling, section-plane state, live tools, first-person
controls, melting, water evaporation, and settled high-explosive fragments.
These checks are not a complete end-to-end audit or a 4K FPS benchmark.

Two defects were fixed: changing the section axis retains the plane's current
world point, and accelerated sand bounds its pending movement to the value
accepted by the motion snapshot loader. A buoyancy test previously equated
presence in the body list with floating; it now verifies that retained stone
has reached the basin floor and is supported.

Known gaps remain. The section view clips terrain and unit meshes; separate
water and fragment meshes are not yet clipped. Unit diagnostic colours expose
available heat, damage, speed and solid state; unavailable values remain grey.
Rolled resting bodies retain their shape,
but their weapon damage and high-energy impact fracture are not implemented
like terrain damage, and retained bodies consume slots in the existing body
limit. Other tilted bodies can still be baked onto the world lattice. The
metric view remains partial for unsupported per-object attributes. Replays
reconstruct some smoke approximately and require compatible simulation data.
