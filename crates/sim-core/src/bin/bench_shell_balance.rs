//! Manual, deterministic shell comparison on the small authored range.
use modding::explosion::ExplosionConfig;
use modding::range::RangeLayout;
use sim_core::blast::BlastRange;

fn main() {
    let stock = ExplosionConfig::from_ron(include_str!("../../../../data/explosion.ron"))
        .expect("stock shell");
    let base = RangeLayout::from_ron(include_str!("../../../../data/range-cover.ron"))
        .expect("stock range");
    println!("scenario,power,impact_tick,world_removed,unit_removed,unit_dents,unit_alive,surface_dents,max_surface_work,world_chunks");
    for scenario in ["open", "covered", "ground"] {
        for power in [500u32, 1000, 2000, 4000] {
            let mut c = stock.clone();
            c.energy = c.energy * power / 1000;
            c.material_energy_j = (u64::from(c.material_energy_j) * u64::from(power) / 1000) as u32;
            c.gas_pressure_milli = (c.gas_pressure_milli * power as i32 / 1000).min(10000);
            c.gas_source_radius = c.gas_source_radius * (1000 + power as i32) / 2000;
            let mut l = base.clone();
            l.course = None;
            l.clear_min = [64, 64];
            l.clear_max = [600, 600];
            l.units.truncate(1);
            l.units[0].at = [300, 16, 336];
            l.target = [286, 20, 336];
            l.blocks.clear();
            if scenario == "covered" {
                l.blocks.push(modding::range::RangeBlock {
                    min: [260, 0, 290],
                    max: [269, 64, 382],
                    material: 3,
                });
            }
            if scenario == "ground" {
                l.units.clear();
                l.target = [286, 15, 336];
            }
            let mut r = BlastRange::with_layout(c, l).expect("scene");
            let mut impact_tick = None;
            for _ in 0..240 {
                r.step();
                if impact_tick.is_none() && r.impact.is_some() {
                    impact_tick = Some(r.tick);
                }
            }
            let unit = r.units.first();
            let unit_removed = unit.map_or(0, |u| u.removed.len() + u.seg_removed.len());
            let unit_dents = unit.map_or(0, |u| u.dents.len());
            let unit_alive = unit.is_some_and(|u| u.status.alive);
            let max_surface_work = r.surface_work.values().copied().max().unwrap_or(0);
            println!(
                "{scenario},{power},{},{},{unit_removed},{unit_dents},{unit_alive},{},{max_surface_work},{}",
                impact_tick.unwrap_or(0),
                r.removed,
                r.surface_work.len(),
                r.world.touched_chunks()
            );
        }
    }
}
