# 444 — Continuous terrain metric display

The earlier diagnostic surface was a 4.8–19.2 m ray-sampled mesh; gaps and
the uncoloured rest of the scene made it look like a checkerboard. In the
laboratory the existing terrain colour pass now directly colours every visible
terrain surface for heat, burning, moisture, deformation, material phase,
strength, density and material identity. This applies in normal and live
camera modes and does not change the world or its voxel size.

For dynamic metrics a sparse 8192-slot open-addressed GPU table contains at
most 4096 non-zero cells, refreshed every five simulation ticks. CPU work and
GPU storage are bounded. On overflow or an overlong collision chain the whole
dynamic terrain layer is grey (unknown), never silently reported as zero.
Static attributes use the current material registry. Some terrain rendering
codes represent multiple physical materials: values are grey if those possible
materials disagree, rather than inventing an ID, strength or density.

Water surface chunks also use the selected metric: phase, wetness, fill and
material are known, while heat, speed, deformation and burning are grey when
not cheaply available. Chunks rebuild under the existing four-per-frame budget.
The pre-existing section-plane mesh remains in use for a slice, while terrain
behind it receives the global colouring. This does not yet implement a fully
global overlay for units, detached bodies or all debris. Terrain speed and
water-fill metrics still use the local sampled window. In particular,
water temperature and fragment thermal values remain unknown. The panel states
the scope and the meaning of grey. These limitations are intentional: the
existing terrain rendering codes and simulation do not expose all attributes
for every rendered mesh, and substituting zeros would misrepresent data.

Hidden, FPS-limited 4K captures checked live burning, live heat, material phase
and the orthographic heat section. Heat on a steel plate remains visible while
the rest of the terrain is coloured continuously; a mild orientation-based
shade retains the shape of equally-valued faces. These captures validate the
GPU shader and picture, not interactive behaviour or representative battle FPS.
