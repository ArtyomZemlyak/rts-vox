# Heat and water

Water takes heat from a direct laser or flame, nearby hot surfaces and units,
fuel fires and the bounded thermal regions of explosions and laser halos. A
direct laser spends its heat on water before it can reach material behind it;
solid terrain blocks the relevant direct and radial interactions. Consumed
water is counted as vapour, and a limited number of short-lived cubic steam
particles provide feedback without creating simulated gas bodies.

Small heat deposits accumulate until they can evaporate a unit of water. The
fractional remainder follows moving free water and is stored in snapshot
wrapper 73; earlier snapshots remain readable. Liquid, absorbed moisture and
vapour remain in the existing water balance. Sub-unit heat on water absorbed
into a porous solid is an approximation and is not tracked separately.

This is an approximation, not a temperature and phase-change solver: deposited
heat goes directly toward evaporation, and vapour has no independent flow or
condensation. Large radial effects have bounded candidate and line-of-sight
budgets; a single pulse does not necessarily cover every cell in a very large
pool. Visual steam has separate particle limits and can omit some contacts.

Focused tests cover direct flame and laser contact, laser attenuation and
continuation, shielding, hot units, fuel fire, thermal pulses, water balance,
fractional heat and snapshot replay. A renderer test checks the steam-particle
limit. No FPS measurement is implied by these tests.
