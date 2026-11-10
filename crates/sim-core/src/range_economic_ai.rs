//! Small deterministic economy policy for sides with a commander.
//! Decisions are reconstructed from the live world, not a second economy.
use super::{battle, motion, BlastRange, RangeUnit, RangeUnitKind};

const INTERVAL: u32 = 60;
const MAX_ARMY: usize = 12;
const DEFENCE_RADIUS: i64 = 1500;

fn combat_unit(u: &RangeUnit) -> bool {
    u.status.alive && u.status.weapon && !u.kind.emplacement()
        && !matches!(u.kind,RangeUnitKind::Commander|RangeUnitKind::Crawler|RangeUnitKind::SkobaDrone)
}

/// Shared by both sides: a nearby hostile matters to the economy only if
/// it can actually fire. Do not treat factories and collectors as threats.
fn armed_hostile(range: &BlastRange, id: usize, side: bool, target: RangeUnitKind) -> Option<i32> {
    let u = range.units.get(id)?;
    let actor = range.battle.actors.get(id)?;
    if !u.status.alive || !u.status.weapon || range.allied(id) == side || range.idle_enemy(id) {
        return None;
    }
    let weapon = battle::effective_weapon(u.kind, actor.weapon, false, Some(target), &range.battle.tuning)?;
    Some(battle::attack_range(&range.battle.tuning, u.kind, weapon, range.streams.tuning.machine_gun.range))
}

impl BlastRange {
    fn economic_factory(&self,side:bool)->Option<usize> {
        self.units.iter().enumerate().filter(|(id,u)|self.allied(*id)==side && u.status.alive && !self.idle_enemy(*id)
            && !self.unit_under_construction(*id) && matches!(u.kind,RangeUnitKind::KilFactoryT1|RangeUnitKind::KilFactoryT2|RangeUnitKind::KilFactoryT3|RangeUnitKind::KilFactoryT4))
            .min_by_key(|(id,_)|(self.production_queue(*id).is_some_and(|q|!q.is_empty()),*id)).map(|(id,_)|id)
    }
    /// The same economy controller can operate an existing industrial base in
    /// scenarios without a commander. It uses only live allied factories,
    /// stores and collectors; scripted idle actors are left alone.
    fn economy_without_commander(&mut self, side: bool) {
        self.assign_ai_crawlers(side);
        let Some(factory)=self.economic_factory(side) else {return;};
        let tank_exists = self.units.iter().enumerate().any(|(id, u)| {
            self.allied(id) == side && u.status.alive && u.kind == RangeUnitKind::Tank
        });
        let dock_exists = self.units.iter().enumerate().any(|(id, u)| {
            self.allied(id) == side && u.status.alive && !self.unit_under_construction(id)
                && matches!(u.kind, RangeUnitKind::Base | RangeUnitKind::GrebenStorage)
        });
        let crawlers = self.units.iter().enumerate().filter(|(id, u)| {
            self.allied(*id) == side && u.status.alive && u.kind == RangeUnitKind::Crawler
        }).count();
        let army = self.units.iter().enumerate().filter(|(id, u)| {
            self.allied(*id) == side && combat_unit(u)
        }).count();
        let policy=self.economic_rules().ai.as_ref();
        let need_miner=crawlers<policy.map_or(1,|p|usize::from(p.mining_crawlers));
        let limit=policy.map_or(MAX_ARMY,|p|usize::from(p.max_combat_units));
        if (army < limit || (tank_exists && need_miner)) && self.production_queue(factory).is_none() {
            let product = if !tank_exists { Some(RangeUnitKind::Tank) }
                else if dock_exists && need_miner { Some(RangeUnitKind::Crawler) }
                else if dock_exists { Some(RangeUnitKind::Tank) }
                else { None };
            if let Some(product) = product { self.enqueue_product(factory, product, side); }
        }
    }

    fn assign_ai_crawlers(&mut self, side: bool) {
        let crawlers: Vec<_> = self.units.iter().enumerate().filter_map(|(id, u)| {
            (self.allied(id) == side && u.status.alive && u.kind == RangeUnitKind::Crawler
                && !self.idle_enemy(id) && !self.hauling(id)).then_some(id)
        }).collect();
        for id in crawlers {
            match self.battle.actors[id].order {
                // Keep the requested column while travelling and while the
                // drill can still cut it. Otherwise recomputing around the
                // moving crawler shifts the destination every AI interval.
                battle::Order::Dig(face) if face[0] >= 0 && face[2] >= 0
                    && (face[0] as u32) < self.world.nx && (face[2] as u32) < self.world.nz
                    && self.world.column_ceiling(face[0] as u32, face[2] as u32) > 1 => continue,
                battle::Order::Reclaim(_) => continue,
                _ => {}
            }
            if let Some(face) = self.economic_face(id) {
                if self.battle.actors[id].order != battle::Order::Dig(face) {
                    self.economy_ai_order(id, battle::Order::Dig(face), side);
                }
            }
        }
    }

    fn economic_face(&self, id: usize) -> Option<[i32; 3]> {
        let at = self.units[id].motion.pos.map(|v| v.to_int_trunc());
        // Do not undermine the actor or any building. Visit nearby columns in
        // fixed order and search below the surface for an actual solid cell.
        for [dx, dz] in [
            [0, 90],
            [0, -90],
            [90, 0],
            [-90, 0],
            [140, 80],
            [-140, 80],
            [140, -80],
            [-140, -80],
        ] {
            let x = at[0] + dx;
            let z = at[2] + dz;
            if x < 2 || z < 2 || x >= self.size[0] as i32 - 2 || z >= self.size[2] as i32 - 2 {
                continue;
            }
            if self.units.iter().any(|u| {
                if !u.status.alive || !u.kind.emplacement() {
                    return false;
                }
                let (lo, hi) = u.bounds();
                x >= lo[0] - 16 && x < hi[0] + 16 && z >= lo[2] - 16 && z < hi[2] + 16
            }) {
                continue;
            }
            // A tree's crown is not the ground. Limit the probe to a short
            // range around the machine and seek an actual solid cell below.
            let top = (motion::floor(&self.world, x, z) - 1)
                .min(at[1] - 1)
                .clamp(1, self.size[1] as i32 - 2);
            for y in ((top - 32).max(1)..=top).rev() {
                let material = self.world.material(x as u32, y as u32, z as u32);
                if material != 0 {
                    return Some([x, y, z]);
                }
            }
        }
        None
    }

    pub(super) fn economic_ai_phase(&mut self) {
        let interval=self.economic_rules().ai.as_ref().map_or(INTERVAL,|p|p.decision_ticks);
        if self.tick % interval != 0
            || self.battle.phase != battle::Phase::Fighting
            || self
                .mining
                .as_ref()
                .and_then(|m| m.data().production.as_ref())
                .is_none()
        {
            return;
        }
        for side in [false, true] {
            if side && !self.battle.autonomous {
                continue;
            }
            let Some(commander) = (0..self.units.len()).find(|&id| {
                self.units[id].kind == RangeUnitKind::Commander
                    && self.units[id].status.alive
                    && self.allied(id) == side
                    && !self.idle_enemy(id)
            }) else {
                self.economy_without_commander(side);
                continue;
            };
            let origin = self.units[commander].motion.pos.map(|v| v.to_int_trunc());
            // Do not insist on economic work while an enemy is in immediate range.
            let threatened = if let Some(enemy) = (0..self.units.len())
                .filter_map(|id| {
                    let reach = armed_hostile(self, id, side, RangeUnitKind::Commander)?;
                    let p = self.units[id].motion.pos;
                    let dx = i64::from(p[0].to_int_trunc() - origin[0]);
                    let dz = i64::from(p[2].to_int_trunc() - origin[2]);
                    let d = dx * dx + dz * dz;
                    // A small keep margin prevents repeatedly abandoning and
                    // resuming construction when a target straddles range.
                    let keep = i64::from(self.battle.actors[commander].order == battle::Order::Attack(id)) * 100;
                    let detect = i64::from(reach).clamp(350, 1200) + keep;
                    (d <= detect * detect).then_some((d, id))
                })
                .min_by_key(|&(d, id)| (d, id))
                .map(|(_, id)| id)
            {
                if self.battle.actors[commander].order != battle::Order::Attack(enemy) {
                    self.economy_ai_order(commander, battle::Order::Attack(enemy), side);
                }
                true
            } else {
                false
            };
            let factory = self.economic_factory(side);
            let policy=self.economic_rules().ai.clone();
            let factory_count=self.units.iter().enumerate().filter(|(id,u)|self.allied(*id)==side && u.status.alive && !self.unit_under_construction(*id)
                && matches!(u.kind,RangeUnitKind::KilFactoryT1|RangeUnitKind::KilFactoryT2|RangeUnitKind::KilFactoryT3|RangeUnitKind::KilFactoryT4)).count();
            let miners=self.units.iter().enumerate().filter(|(id,u)|self.allied(*id)==side && u.status.alive && u.kind==RangeUnitKind::Crawler).count();
            let fighters=self.units.iter().enumerate().filter(|(id,u)|self.allied(*id)==side && combat_unit(u)).count();
            let available=self.commander_accessible_matter(commander);
            let shortage=policy.as_ref().is_some_and(|p|factory.and_then(|id|self.construction_power_networks().get(id).copied().flatten())
                .is_some_and(|(g,d)|d>0 && u128::from(g)*100<u128::from(d)*u128::from(p.min_power_percent)));
            let tank_exists = self.units.iter().enumerate().any(|(id, u)| {
                self.allied(id) == side && u.status.alive && u.kind == RangeUnitKind::Tank
            });
            let dock_exists = self.units.iter().enumerate().any(|(id, u)| {
                self.allied(id) == side && u.status.alive
                    && matches!(u.kind, RangeUnitKind::Base | RangeUnitKind::GrebenStorage)
                    && !self.unit_under_construction(id)
            });
            if !threatened {
                if let Some(factory) = factory {
                    let ready = self.production_progress(factory).is_some_and(|(ticks, needed, paid, price)| ticks >= needed && paid >= price)
                        && self.production_queue(factory).and_then(|q| q.first().copied())
                            .is_some_and(|kind| self.free_spot(factory, kind, side).is_none());
                    if ready {
                        let f = &self.units[factory];
                        let (lo, hi) = f.bounds();
                        let (clo, chi) = self.units[commander].bounds();
                        let fx = f.pose.x.x.raw();
                        let fz = f.pose.x.z.raw();
                        let in_lane = if fx.abs() >= fz.abs() {
                            clo[2] < hi[2] && lo[2] < chi[2] && (i64::from(origin[0] - f.at[0]) * fx < 0)
                                && (origin[0] - f.at[0]).abs() < 800
                        } else {
                            clo[0] < hi[0] && lo[0] < chi[0] && (i64::from(origin[2] - f.at[2]) * fz < 0)
                                && (origin[2] - f.at[2]).abs() < 800
                        };
                        if in_lane {
                            let own = motion::Obstacle::of(&self.units[commander].motion, &self.units[commander].body);
                            let building = motion::Obstacle::of(&f.motion, &f.body);
                            if own.penetration(&building) > fx::Fx::from_int(2) {
                                // A path search cannot start inside a hull. Back
                                // straight out first; normal collision checks
                                // accept only steps that reduce penetration.
                                let heading = self.units[commander].motion.drive.head;
                                let dx = i64::from(origin[0] - f.at[0]);
                                let dz = i64::from(origin[2] - f.at[2]);
                                let dot = i128::from(dx) * i128::from(fx::angle::cos(heading).raw())
                                    + i128::from(dz) * i128::from(fx::angle::sin(heading).raw());
                                let throttle = if dot >= 0 { 1 } else { -1 };
                                let aim = [origin[0].clamp(0, self.size[0] as i32 - 1),
                                    origin[1].clamp(0, self.size[1] as i32 - 1), origin[2].clamp(0, self.size[2] as i32 - 1)];
                                let order = battle::Order::Pilot { throttle, turn: 0, aim, fire: false };
                                if self.battle.actors[commander].order != order {
                                    self.economy_ai_order(commander, order, side);
                                }
                                continue;
                            }
                            if self.battle.actors[commander].order != battle::Order::Move
                                || self.units[commander].motion.destination.is_none() {
                                let mut to = [origin[0], origin[2]];
                                let axis = if fx.abs() >= fz.abs() { 1 } else { 0 };
                                let mid = if axis == 0 { f.at[0] } else { f.at[2] };
                                let sign = if to[axis] >= mid { 1 } else { -1 };
                                let max = self.size[if axis == 0 { 0 } else { 2 }] as i32 - 100;
                                to[axis] = (to[axis] + sign * 300).clamp(100, max.max(100));
                                // Back away from the gate as well as moving
                                // sideways; a large commander can otherwise
                                // begin its route already touching the factory.
                                let forward_axis = 1 - axis;
                                let forward = if forward_axis == 0 { fx } else { fz };
                                let max_forward = self.size[if forward_axis == 0 { 0 } else { 2 }] as i32 - 100;
                                to[forward_axis] = (to[forward_axis] - 240 * forward.signum() as i32).clamp(100, max_forward.max(100));
                                self.economy_ai_move(commander, to, side);
                            }
                            continue;
                        }
                    }
                }
            }
            if !threatened {
                let sites = self.construction_sites();
                let turret_exists = self.units.iter().enumerate().any(|(id, u)| {
                    self.allied(id) == side
                        && u.status.alive
                        && u.kind == RangeUnitKind::TurretT1
                        && !self.unit_under_construction(id)
                });
                let pending=sites.iter().find(|s|!s.complete && self.can_commander_build(commander,s.id));
                let desired = if let Some(project)=pending {
                    Some(project.kind)
                } else if factory.is_none() {
                    Some(RangeUnitKind::KilFactoryT1)
                } else if tank_exists && !turret_exists {
                    Some(RangeUnitKind::TurretT1)
                } else if tank_exists && !dock_exists {
                    // A crawler without a dock can fill up but never deliver.
                    // The base also supplies the construction power that the
                    // starting commander alone cannot provide at full rate.
                    Some(RangeUnitKind::Base)
                } else if shortage {
                    Some(RangeUnitKind::PowerPlantT1)
                } else if policy.as_ref().is_some_and(|p|factory_count<usize::from(p.max_factories)
                    && miners>=usize::from(p.mining_crawlers) && fighters>=usize::from(p.expansion_combat_units))
                    && available>=self.construction_cost(RangeUnitKind::KilFactoryT1).unwrap_or(u64::MAX)
                        .saturating_add(self.production_cost(RangeUnitKind::Tank).unwrap_or(0)) {
                    Some(RangeUnitKind::KilFactoryT1)
                } else { None };
                let mut site = sites
                    .iter()
                    .find(|s| {
                        Some(s.kind) == desired
                            && !s.complete
                            && self.can_commander_build(commander, s.id)
                    })
                    .map(|s| s.id);
                if let Some(kind) = desired
                    .filter(|_| site.is_none() && self.commander_tool_tip(commander).is_some())
                {
                    // Bounded candidate search. Placement remains authoritative.
                    let safe = if side { -1 } else { 1 };
                    for [dx, dz] in [
                        // Fits the factory beside the commander and remains
                        // within 400 units of the construction tool. The first
                        // choice is in front of its initial facing.
                        [-safe * 400, 0],
                        [safe * 400, 0],
                        [0, 360],
                        [0, -360],
                        [safe * 300, 0],
                        [safe * 440, 0],
                        [-safe * 300, 0],
                        [0, 300],
                        [0, -300],
                        [-safe * 440, 0],
                        [0, 440],
                        [0, -440],
                    ] {
                        let x = origin[0] + dx;
                        let z = origin[2] + dz;
                        if x < 1
                            || z < 1
                            || x >= self.size[0] as i32 - 1
                            || z >= self.size[2] as i32 - 1
                        {
                            continue;
                        }
                        let at = [x, motion::floor(&self.world, x, z), z];
                        if let Some(id) = self.create_construction_site(kind, side, at) {
                            self.set_construction_quota(id, 0);
                            site = Some(id);
                            break;
                        }
                    }
                }
                let cap=self.tank_fill(commander).map_or(0,|(_,cap)|cap);
                // Accumulate a complete project's reserve before committing.
                // Once printing starts keep the order through a short shortage,
                // rather than oscillating the tool between dig and print.
                if let Some(site) = site.filter(|&site| self.battle.actors[commander].order==battle::Order::Build(site) || self.construction_sites().iter().find(|s| s.id==site)
                    .is_some_and(|s| available >= s.required.saturating_sub(s.delivered).min(cap))) {
                    if self.battle.actors[commander].order != battle::Order::Build(site) {
                        self.economy_ai_order(commander, battle::Order::Build(site), side);
                    }
                } else if let Some(face) = self.economic_face(commander) {
                    if self.battle.actors[commander].order != battle::Order::Dig(face) {
                        self.economy_ai_order(commander, battle::Order::Dig(face), side);
                    }
                }
            }
            self.assign_ai_crawlers(side);
            let Some(factory) = factory else {
                continue;
            };
            let army = self
                .units
                .iter()
                .enumerate()
                .filter(|(id, u)| {
                    self.allied(*id) == side && combat_unit(u)
                })
                .count();
            let defensive_need = self.units.iter().enumerate().any(|(id, u)| {
                if armed_hostile(self, id, side, RangeUnitKind::KilFactoryT1).is_none() { return false; }
                let pos = u.motion.pos;
                let f = self.units[factory].motion.pos;
                let dx = i64::from(pos[0].to_int_trunc() - f[0].to_int_trunc());
                let dz = i64::from(pos[2].to_int_trunc() - f[2].to_int_trunc());
                let reach=policy.as_ref().map_or(DEFENCE_RADIUS,|p|i64::from(p.defence_radius));
                dx * dx + dz * dz <= reach * reach
            });
            let field_tanks = self.units.iter().enumerate().filter(|(id, u)| {
                self.allied(*id) == side && u.status.alive && u.kind == RangeUnitKind::Tank
            }).count();
            let need_defender = defensive_need && field_tanks < 2;
            let queue = self.production_queue(factory);
            // A saved battle can already have an unfinished crawler when its
            // tank or its dock was lost. Refund it so the side can recover.
            if (!tank_exists || !dock_exists || need_defender)
                && queue
                    .as_ref()
                    .is_some_and(|q| q.len() == 1 && q[0] == RangeUnitKind::Crawler)
            {
                self.cancel_product_preserving_matter(factory, commander, side);
            }
            // Reserve the commander's mined mass for the first dock instead
            // of tying it up in a crawler that has nowhere to unload.
            let max_army=policy.as_ref().map_or(MAX_ARMY,|p|usize::from(p.max_combat_units));
            let need_miner=miners<policy.as_ref().map_or(1,|p|usize::from(p.mining_crawlers));
            let project=self.construction_sites().iter().any(|s|!s.complete && self.can_commander_build(commander,s.id));
            let can_spend=need_defender || !tank_exists || need_miner
                || (!project && available>=self.production_cost(RangeUnitKind::Tank).unwrap_or(u64::MAX));
            if (army < max_army || (tank_exists && need_miner && !need_defender)) && can_spend && self.production_queue(factory).is_none()
                && (!tank_exists || dock_exists || need_defender) {
                let product = if !tank_exists || need_defender {
                    RangeUnitKind::Tank
                } else if need_miner {
                    RangeUnitKind::Crawler
                } else {
                    RangeUnitKind::Tank
                };
                self.enqueue_product(factory, product, side);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use modding::range::{RangeBlock, RangeLayout, RangeUnitDef};

    #[test]
    fn battle_room_handicap_initialises_each_side_and_survives_save() {
        let mut layout = modding::laboratory::Definition::stock().layout;
        layout.size = [1024, 256, 1024];
        layout.clear_max = [1023, 1023];
        layout.battle = true;
        layout.units = vec![
            RangeUnitDef { kind: RangeUnitKind::Commander, allied: true, at: [220, 16, 220], weapon: None },
            RangeUnitDef { kind: RangeUnitKind::Commander, allied: false, at: [804, 16, 804], weapon: None },
        ];
        layout.economy = Some(modding::range_economy::RangeEconomy::stock());
        layout.ready = Some(modding::range::RangeReady { commander_stock: 25_000_000, enemy_commander_stock: Some(75_000_000), ..Default::default() });
        let config = modding::explosion::ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
        let r = BlastRange::with_layout(config, layout).unwrap();
        assert_eq!(r.tank_held(0), Some(25_000_000));
        assert_eq!(r.tank_held(1), Some(75_000_000));
        assert!(r.matter.economy.ledger.balanced());
        let copy = BlastRange::restore(r.config, &r.snapshot()).expect("handicap save restores");
        assert_eq!(copy.tank_held(0), Some(25_000_000));
        assert_eq!(copy.tank_held(1), Some(75_000_000));
        assert_eq!(copy.state_hash(), r.state_hash());
    }

    fn battle_layout() -> RangeLayout {
        let mut layout =
            RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
        layout.battle = true;
        layout.idle_enemies = false;
        layout.size = [3200, 256, 3200];
        layout.clear_min = [1, 1];
        layout.clear_max = [3199, 3199];
        layout.blocks = vec![
            RangeBlock {
                min: [0, 0, 0],
                max: [3200, 256, 3200],
                material: 0,
            },
            RangeBlock {
                min: [0, 0, 0],
                max: [3200, 16, 3200],
                material: 1,
            },
        ];
        layout.course = None;
        layout.shields.clear();
        layout.units = vec![
            RangeUnitDef {
                kind: RangeUnitKind::Commander,
                allied: true,
                at: [900, 16, 1600],
                weapon: None,
            },
            RangeUnitDef {
                kind: RangeUnitKind::Commander,
                allied: false,
                at: [2300, 16, 1600],
                weapon: None,
            },
        ];
        layout.economy = Some(modding::range_economy::RangeEconomy::stock());
        layout
    }

    fn battle() -> BlastRange {
        let layout = battle_layout();
        let cfg = modding::explosion::ExplosionConfig::from_ron(include_str!(
            "../../../data/energy-cannon.ron"
        ))
        .unwrap();
        let mut r = BlastRange::with_layout(cfg, layout).unwrap();
        r.set_physics_workers(1);
        r.set_navigation_workers(1);
        assert!(r.set_ai_vs_ai(true));
        r.start_battle().unwrap();
        r
    }

    #[test]
    fn economy_followup_support_crew_does_not_cap_army_and_expansion_keeps_its_reserve() {
        let mut r = battle();
        // Begin dynamic placement before funding so new objects cannot reset
        // the ledger as an authored layout would.
        r.place_sandbox_unit(RangeUnitKind::PowerPlantT1, true, [600,16,1100], 0).unwrap();
        r.place_sandbox_unit(RangeUnitKind::Base, true, [600,16,2000], 0).unwrap();
        let factory=r.place_sandbox_unit(RangeUnitKind::KilFactoryT1,true,[1340,16,1600],0).unwrap();
        r.place_sandbox_unit(RangeUnitKind::TurretT1,true,[1000,16,1000],0).unwrap();
        for x in [700,1500] {
            r.place_sandbox_unit(RangeUnitKind::Crawler,true,[x,16,2500],0).unwrap();
        }
        for i in 0..8 {
            r.place_sandbox_unit(RangeUnitKind::SkobaDrone,true,[300+i*200,100,300],0).unwrap();
        }
        r.place_sandbox_unit(RangeUnitKind::Tank,true,[250,16,2600],0).unwrap();
        r.units[1].status.weapon=false;
        let tank=r.mining().unwrap().tank(0).unwrap();
        r.matter.economy.ledger.generate(tank,100_000_000);
        r.economic_ai_phase();
        assert_eq!(r.production_queue(factory),Some(vec![RangeUnitKind::Tank]),
            "eight builders, two miners and a commander must not consume the combat cap");
        r.place_sandbox_unit(RangeUnitKind::Tank,true,[250,16,2800],0).unwrap();
        r.tick=INTERVAL;
        r.economic_ai_phase();
        let project=r.construction_sites().into_iter().find(|s|!s.complete && s.kind==RangeUnitKind::KilFactoryT1
            && r.can_commander_build(0,s.id)).expect("funded economy should expand after two combat units and two miners");
        assert_eq!(r.battle.actors[0].order,battle::Order::Build(project.id));
        // The starting threshold is no longer available once a project has
        // consumed resources; this must not cancel the already accepted order.
        r.matter.economy.ledger.transfer(tank,r.matter.economy.ground,30_000_000);
        r.tick=INTERVAL*2;
        r.economic_ai_phase();
        assert_eq!(r.battle.actors[0].order,battle::Order::Build(project.id));
        assert_eq!(r.construction_sites().iter().filter(|s|s.kind==RangeUnitKind::KilFactoryT1 && !s.complete
            && r.can_commander_build(0,s.id)).count(),1);
        assert!(r.matter.economy.ledger.balanced());
    }

    #[test]
    fn economy_balance_ai_reserves_projects_and_prints_after_resupply() {
        let mut r=battle();
        let own=r.mining.as_ref().unwrap().tank(0).unwrap();
        let enemy=r.mining.as_ref().unwrap().tank(1).unwrap();
        r.matter.economy.ledger.generate(own,50_000_000);
        r.matter.economy.ledger.generate(enemy,100_000_000);
        r.economic_ai_phase();
        assert!(matches!(r.battle.actors[0].order,battle::Order::Dig(_)),"poor side must accumulate, {:?}",r.battle.actors[0].order);
        assert!(matches!(r.battle.actors[1].order,battle::Order::Build(_)),"stocked side must start, {:?}",r.battle.actors[1].order);
        for _ in 0..INTERVAL { r.step(); }
        assert!(r.construction_sites().iter().any(|s|s.delivered>0),"stocked commander never printed");
        r.matter.economy.ledger.generate(own,100_000_000);
        r.economic_ai_phase();
        assert!(matches!(r.battle.actors[0].order,battle::Order::Build(_)),"resupplied side must commit its reserve");
        assert!(r.matter.economy.ledger.balanced());
        let copy=BlastRange::restore(r.config,&r.snapshot()).expect("real battle economy restores");
        assert_eq!(copy.state_hash(),r.state_hash());
    }

    #[test]
    fn autonomous_commanders_plan_factories_and_mine_real_ground() {
        let mut r = battle();
        r.economic_ai_phase();
        assert_eq!(r.construction_sites().len(), 2);
        assert!(r
            .battle
            .actors
            .iter()
            .all(|a| matches!(a.order, battle::Order::Dig(_))));
        for _ in 0..30 {
            r.step();
        }
        assert!(r.tank_held(0).unwrap() > 0);
        assert!(r.tank_held(1).unwrap() > 0);
        assert!(r.matter.economy.ledger.balanced());
        let mut restored =
            BlastRange::restore(r.config, &r.snapshot()).expect("economy AI snapshot");
        // Cross the next economy decision after restoring, including the
        // transition from mining into construction.
        for _ in 0..35 {
            r.step();
            restored.step();
        }
        assert_eq!(r.state_hash(), restored.state_hash());
        for _ in 0..90 {
            r.step();
        }
        let sites = r.construction_sites();
        assert!(sites.iter().any(|s| s.delivered > 0), "both AIs mined but neither began printing: {sites:?}, tanks {:?}, orders {:?}, positions {:?}", [r.tank_held(0),r.tank_held(1)], [r.battle.actors[0].order,r.battle.actors[1].order], [r.units[0].motion.pos,r.units[1].motion.pos]);
        assert!(r.matter.economy.ledger.balanced());
    }

    #[test]
    fn commander_clears_factory_gate_after_order_is_fully_paid() {
        let mut r = battle();
        let factory = r.place_sandbox_unit(RangeUnitKind::KilFactoryT1, true, [1340, 16, 1600], 0).unwrap();
        r.units[0].motion.pos[0] = fx::Fx::from_int(1000);
        let head = r.units[0].motion.drive.head;
        r.units[0].update_pose(head);
        let cost = r.production_cost(RangeUnitKind::Tank).unwrap();
        let tank = r.mining().unwrap().tank(0).unwrap();
        let ground = r.matter.economy.ground;
        r.matter.economy.ledger.reveal(ground, cost);
        assert_eq!(r.matter.economy.ledger.transfer(ground, tank, cost), cost);
        assert!(r.enqueue_product(factory, RangeUnitKind::Tank, true).accepted());
        for _ in 0..250 { r.production_phase(); }
        let (ticks, needed, paid, price) = r.production_progress(factory).expect("blocked vehicle remains queued");
        assert!(ticks >= needed && paid == price);
        assert!(r.free_spot(factory, RangeUnitKind::Tank, true).is_none());
        r.tick = INTERVAL;
        r.economic_ai_phase();
        assert!(matches!(r.battle.actors[0].order, battle::Order::Move | battle::Order::Pilot { .. }));
        for _ in 0..600 {
            r.step();
            if r.units.iter().enumerate().any(|(id, u)| r.allied(id) && u.kind == RangeUnitKind::Tank && u.status.alive) { break; }
        }
        assert!(r.units.iter().enumerate().any(|(id, u)| r.allied(id) && u.kind == RangeUnitKind::Tank && u.status.alive),
            "commander failed to clear gate: at {:?}, order {:?}, destination {:?}, path {:?}, drive {:?}, blocked {}, status {:?}, planning {:?}, overlap {:?}, queue {:?}",
            r.units[0].at, r.battle.actors[0].order, r.units[0].motion.destination, r.units[0].motion.path, r.units[0].motion.drive, r.units[0].motion.blocked,
            r.units[0].status, r.units[0].motion.last_plan,
            motion::Obstacle::of(&r.units[0].motion, &r.units[0].body).penetration(&motion::Obstacle::of(&r.units[factory].motion, &r.units[factory].body)), r.production_queue(factory));
        assert!(r.matter.economy.ledger.balanced());
    }

    #[test]
    fn autonomous_factory_orders_a_tank_and_uses_nearby_commander_stock() {
        let mut r = battle();
        let factory = r
            .place_sandbox_unit(RangeUnitKind::KilFactoryT1, true, [1340, 16, 1600], 0)
            .unwrap();
        r.units[0].motion.pos[0] = fx::Fx::from_int(1000);
        let head = r.units[0].motion.drive.head;
        r.units[0].update_pose(head);
        let tank = r.mining().unwrap().tank(0).unwrap();
        let ground = r.matter.economy.ground;
        let amount = 1_000_000;
        r.matter.economy.ledger.reveal(ground, amount);
        assert_eq!(
            r.matter.economy.ledger.transfer(ground, tank, amount),
            amount
        );
        r.economic_ai_phase();
        assert_eq!(r.production_queue(factory), Some(vec![RangeUnitKind::Tank]));
        r.production_phase();
        assert!(r.production_paid(factory).unwrap() > 0);
        assert!(r.tank_held(0).unwrap() < amount);
        assert!(r.matter.economy.ledger.balanced());
    }

    #[test]
    fn ai_replaces_lost_tank_before_finishing_expensive_crawler() {
        let mut r = battle();
        let factory = r
            .place_sandbox_unit(RangeUnitKind::KilFactoryT1, true, [1340, 16, 1600], 0)
            .unwrap();
        let tank = r
            .place_sandbox_unit(RangeUnitKind::Tank, true, [600, 16, 2000], 0)
            .unwrap();
        r.units[0].motion.pos[0] = fx::Fx::from_int(1000);
        let head = r.units[0].motion.drive.head;
        r.units[0].update_pose(head);
        assert!(r
            .enqueue_product(factory, RangeUnitKind::Crawler, true)
            .accepted());
        // Partial production must be refunded, never silently discarded.
        let store = r.mining().unwrap().haul().unwrap().store(true);
        let ground = r.matter.economy.ground;
        r.matter.economy.ledger.reveal(ground, 100_000);
        assert_eq!(
            r.matter.economy.ledger.transfer(ground, store, 100_000),
            100_000
        );
        let commander_tank = r.mining().unwrap().tank(0).unwrap();
        r.matter.economy.ledger.reveal(ground, 1_000_000);
        assert_eq!(
            r.matter
                .economy
                .ledger
                .transfer(ground, commander_tank, 1_000_000),
            1_000_000
        );
        r.production_phase();
        assert!(r.production_paid(factory).unwrap_or(0) > 350_000);
        r.units[tank].status.alive = false;
        r.units[1].motion.pos = [1000, 16, 1800].map(fx::Fx::from_int);
        let head = r.units[1].motion.drive.head;
        r.units[1].update_pose(head);
        r.economic_ai_phase();
        assert_eq!(r.battle.actors[0].order, battle::Order::Attack(1));
        assert_eq!(r.production_queue(factory), Some(vec![RangeUnitKind::Tank]));
        assert_eq!(r.mining().unwrap().production().unwrap().overflow, 0);
        assert!(r.matter.economy.ledger.balanced());
    }

    #[test]
    fn mining_face_avoids_tree_crowns_and_building_foundations() {
        let mut r = battle();
        let factory = r
            .place_sandbox_unit(RangeUnitKind::KilFactoryT1, true, [1340, 16, 1600], 0)
            .unwrap();
        for y in 16..55 {
            r.world.queue_edit(voxel::Edit::put(900, y, 1690, 5));
        }
        r.world.flush();
        let face = r.economic_face(0).expect("ground within reach");
        assert_eq!(face, [900, 15, 1690]);
        let (lo, hi) = r.units[factory].bounds();
        assert!(
            face[0] < lo[0] - 16
                || face[0] >= hi[0] + 16
                || face[2] < lo[2] - 16
                || face[2] >= hi[2] + 16
        );
    }

    #[test]
    fn commander_start_stock_and_objective_round_trip() {
        let original = battle();
        let mut layout = original.layout.clone().unwrap();
        layout.ready = Some(modding::range::RangeReady {
            commander_stock: 50_000_000,
            ..Default::default()
        });
        let mut r = BlastRange::with_layout(original.config, layout).unwrap();
        assert_eq!(r.tank_held(0), Some(50_000_000));
        assert_eq!(r.tank_held(1), Some(50_000_000));
        assert!(r.matter.economy.ledger.balanced());
        let copy = BlastRange::restore(r.config, &r.snapshot()).expect("starting stock restores");
        assert_eq!(r.state_hash(), copy.state_hash());
        r.start_battle().unwrap();
        r.units[1].status.alive = false;
        r.battle_tick();
        assert_eq!(r.battle.phase, battle::Phase::Victory);
    }

    #[test]
    fn commander_builds_a_defensive_turret_after_the_first_tank() {
        let mut r = battle();
        r.place_sandbox_unit(RangeUnitKind::KilFactoryT1, true, [1340, 16, 1600], 0)
            .unwrap();
        r.place_sandbox_unit(RangeUnitKind::Tank, true, [600, 16, 2000], 0)
            .unwrap();
        let tank = r.mining().unwrap().tank(0).unwrap();
        let ground = r.matter.economy.ground;
        r.matter.economy.ledger.reveal(ground, 100_000_000);
        assert_eq!(
            r.matter.economy.ledger.transfer(ground, tank, 100_000_000),
            100_000_000
        );
        r.economic_ai_phase();
        let site = r
            .construction_sites()
            .into_iter()
            .find(|s| s.kind == RangeUnitKind::TurretT1 && r.valid_commander_site(0, s.id))
            .expect("AI turret site");
        assert_eq!(r.battle.actors[0].order, battle::Order::Build(site.id));
        for _ in 0..300 {
            r.step();
            if r.construction_sites()[site.id].complete {
                break;
            }
        }
        assert!(
            r.construction_sites()[site.id].complete,
            "turret did not complete: site {:?}, order {:?}, position {:?}, tank {:?}",
            r.construction_sites()[site.id],
            r.battle.actors[0].order,
            r.units[0].motion.pos,
            r.tank_held(0)
        );
        assert!(r.matter.economy.ledger.balanced());
    }

    #[test]
    fn ai_builds_a_dock_before_ordering_its_crawler() {
        let mut r = battle();
        let factory = r.place_sandbox_unit(RangeUnitKind::KilFactoryT1, true, [1340, 16, 1600], 0).unwrap();
        r.place_sandbox_unit(RangeUnitKind::Tank, true, [600, 16, 2000], 0).unwrap();
        r.place_sandbox_unit(RangeUnitKind::TurretT1, true, [600, 16, 1200], 0).unwrap();
        let tank = r.mining().unwrap().tank(0).unwrap();
        let ground = r.matter.economy.ground;
        r.matter.economy.ledger.reveal(ground, 100_000_000);
        assert_eq!(r.matter.economy.ledger.transfer(ground, tank, 100_000_000), 100_000_000);
        r.economic_ai_phase();
        let site = r.construction_sites().into_iter()
            .find(|s| s.kind == RangeUnitKind::Base && r.valid_commander_site(0, s.id))
            .expect("the AI must begin a real unloading base");
        assert_eq!(r.battle.actors[0].order, battle::Order::Build(site.id));
        assert!(r.production_queue(factory).is_none(), "crawler must wait for a dock");
        r.place_sandbox_unit(RangeUnitKind::Base, true, [1600, 16, 2200], 0).unwrap();
        r.tick = INTERVAL;
        r.economic_ai_phase();
        assert_eq!(r.production_queue(factory), Some(vec![RangeUnitKind::Crawler]));
        assert!(r.matter.economy.ledger.balanced());
    }

    #[test]
    fn ai_refunds_a_crawler_when_its_dock_is_lost() {
        let mut r = battle();
        let factory = r.place_sandbox_unit(RangeUnitKind::KilFactoryT1, true, [1340, 16, 1600], 0).unwrap();
        r.place_sandbox_unit(RangeUnitKind::Tank, true, [600, 16, 2000], 0).unwrap();
        r.place_sandbox_unit(RangeUnitKind::TurretT1, true, [600, 16, 1200], 0).unwrap();
        r.units[0].motion.pos[0] = fx::Fx::from_int(1000);
        let head = r.units[0].motion.drive.head;
        r.units[0].update_pose(head);
        let tank = r.mining().unwrap().tank(0).unwrap();
        let ground = r.matter.economy.ground;
        r.matter.economy.ledger.reveal(ground, 10_000_000);
        assert_eq!(r.matter.economy.ledger.transfer(ground, tank, 10_000_000), 10_000_000);
        assert!(r.enqueue_product(factory, RangeUnitKind::Crawler, true).accepted());
        r.production_phase();
        assert!(r.production_paid(factory).unwrap_or(0) > 0);
        r.economic_ai_phase();
        assert!(r.production_queue(factory).is_none());
        assert!(r.construction_sites().iter().any(|s| s.kind == RangeUnitKind::Base));
        assert!(r.matter.economy.ledger.balanced());
    }

    #[test]
    fn ai_defends_against_armed_threats_and_resumes_economy() {
        let mut r = battle();
        let factory = r.place_sandbox_unit(RangeUnitKind::KilFactoryT1, true, [1340, 16, 1600], 0).unwrap();
        r.place_sandbox_unit(RangeUnitKind::Tank, true, [600, 16, 2000], 0).unwrap();
        let enemy = r.place_sandbox_unit(RangeUnitKind::Tank, false, [1450, 16, 1900], 0).unwrap();
        // The other commander is also within the factory's defence radius;
        // disable its weapon to isolate the approaching test tank.
        r.units[1].status.weapon = false;
        let tank = r.mining().unwrap().tank(0).unwrap();
        let ground = r.matter.economy.ground;
        r.matter.economy.ledger.reveal(ground, 10_000_000);
        assert_eq!(r.matter.economy.ledger.transfer(ground, tank, 10_000_000), 10_000_000);
        r.units[enemy].status.weapon = false;
        r.economic_ai_phase();
        assert!(matches!(r.battle.actors[0].order, battle::Order::Build(_) | battle::Order::Dig(_)));
        assert!(r.production_queue(factory).is_none(), "unarmed hull must not trigger emergency production");
        r.units[enemy].status.weapon = true;
        r.tick = INTERVAL;
        r.economic_ai_phase();
        assert_eq!(r.battle.actors[0].order, battle::Order::Attack(enemy));
        assert_eq!(r.production_queue(factory), Some(vec![RangeUnitKind::Tank]));
        r.units[enemy].motion.pos = [2600, 16, 2400].map(fx::Fx::from_int);
        let head = r.units[enemy].motion.drive.head;
        r.units[enemy].update_pose(head);
        r.tick = INTERVAL * 2;
        r.economic_ai_phase();
        assert!(matches!(r.battle.actors[0].order, battle::Order::Build(_) | battle::Order::Dig(_)));
        assert!(r.matter.economy.ledger.balanced());
    }

    #[test]
    fn ai_crawler_keeps_its_drill_target_while_the_machine_moves() {
        let mut r = battle();
        let crawler = r.place_sandbox_unit(RangeUnitKind::Crawler, true, [1600, 16, 2200], 0).unwrap();
        r.assign_ai_crawlers(true);
        let order = r.battle.actors[crawler].order;
        assert!(matches!(order, battle::Order::Dig(_)));
        r.units[crawler].motion.pos[0] = r.units[crawler].motion.pos[0].add(fx::Fx::from_int(32));
        r.units[crawler].update_pose(0);
        r.assign_ai_crawlers(true);
        assert_eq!(r.battle.actors[crawler].order, order, "AI shifted an active drill target");
    }

    #[test]
    fn existing_industry_works_without_commander_but_never_controls_player_side() {
        let mut r = battle();
        let factory = r.place_sandbox_unit(RangeUnitKind::KilFactoryT1, true, [1340, 16, 1600], 0).unwrap();
        r.units[0].status.alive = false;
        r.battle.autonomous = false;
        r.economic_ai_phase();
        assert!(r.production_queue(factory).is_none());
        r.battle.autonomous = true;
        r.tick = INTERVAL;
        r.economic_ai_phase();
        assert_eq!(r.production_queue(factory), Some(vec![RangeUnitKind::Tank]));
        let crawler = r.place_sandbox_unit(RangeUnitKind::Crawler, true, [1600, 16, 2200], 0).unwrap();
        r.units[factory].status.alive = false;
        r.tick = INTERVAL * 2;
        r.economic_ai_phase();
        assert!(matches!(r.battle.actors[crawler].order, battle::Order::Dig(_)),
            "collectors must keep working even without a factory or commander");
        assert!(r.matter.economy.ledger.balanced());
    }

    #[test]
    fn commanders_build_factories_and_start_production_in_a_real_battle() {
        let mut layout = battle_layout();
        // Separate economic openings from early commander duels on the old
        // 140 m fixture. Both sides still run the complete live battle tick.
        layout.size=[5600,super::super::placement::catalogue_height(16).max(512),2304];
        layout.clear_max=[5599,2303];
        for block in &mut layout.blocks {
            block.max[0]=5600; block.max[2]=2304;
            if block.material==0 {block.max[1]=layout.size[1] as i32;}
        }
        layout.units[0].at=[1000,16,1152]; layout.units[1].at=[4600,16,1152];
        // Only the opening factory and tank are exercised here. Keep live
        // stock costs/rates/AI, without unrelated T3/T4 exit-mask preparation.
        let producers = &mut layout.economy.as_mut().unwrap().production.as_mut().unwrap().producers;
        producers.retain(|p| p.kind == RangeUnitKind::KilFactoryT1);
        producers[0].produces = vec![RangeUnitKind::Tank,RangeUnitKind::Crawler];
        layout.ready = Some(modding::range::RangeReady {
            commander_stock: modding::range_economy::Balance::stock().initial_matter,
            ..Default::default()
        });
        let cfg = modding::explosion::ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
        let mut r = BlastRange::with_layout(cfg, layout).unwrap();
        r.set_physics_workers(1);
        r.set_navigation_workers(1);
        assert!(r.set_ai_vs_ai(true));
        r.start_battle().unwrap();
        // The commander now pays for every cell and its own generation
        // supplies only part of full printing power. Allow enough simulated
        // time for two full factories and their first tank to complete.
        let mut opening = [[None;2];2];
        for _ in 0..1400 {
            r.step();
            for (id,u) in r.units.iter().enumerate() {
                if !u.status.alive || r.unit_under_construction(id) {continue;}
                let column = match u.kind {RangeUnitKind::KilFactoryT1=>0,RangeUnitKind::Tank=>1,_=>continue};
                let side = usize::from(!r.allied(id));
                if opening[side][column].is_none() {opening[side][column]=Some(r.tick);}
            }
            if (0..2).all(|side| {
                r.units.iter().enumerate().any(|(id, u)| {
                    r.allied(id) == (side == 0) && u.kind == RangeUnitKind::Tank && u.status.alive
                })
            }) {
                break;
            }
        }
        eprintln!("PACING real AI opening [factory,tank] ticks: {opening:?}");
        if opening.iter().any(|s|s[1].is_none()) {
            for (id,u) in r.units.iter().enumerate() {
                eprintln!("blocked opening: id={id} kind={:?} bounds={:?} alive={} order={:?}",
                    u.kind,u.bounds(),u.status.alive,r.battle.actors[id].order);
            }
        }
        for side in [true, false] {
            let factory = r
                .units
                .iter()
                .enumerate()
                .find(|(id, u)| {
                    r.allied(*id) == side
                        && u.kind == RangeUnitKind::KilFactoryT1
                        && !r.unit_under_construction(*id)
                })
                .map(|(id, _)| id)
                .unwrap_or_else(|| {
                    panic!(
                        "side {side}: sites {:?}, orders {:?}",
                        r.construction_sites(),
                        r.battle.actors.iter().map(|a| a.order).collect::<Vec<_>>()
                    )
                });
            assert!(
                r.units
                    .iter()
                    .enumerate()
                    .any(|(id, u)| r.allied(id) == side && u.kind == RangeUnitKind::Tank),
                "side {side}: factory {factory} has not produced a tank, queue {:?}, paid {:?}, progress {:?}, stock {:?}, positions {:?}, orders {:?}",
                r.production_queue(factory),
                r.production_paid(factory),
                r.production_progress(factory),
                [r.tank_held(0), r.tank_held(1)],
                [r.units[0].motion.pos, r.units[1].motion.pos, r.units[factory].motion.pos],
                [r.battle.actors[0].order, r.battle.actors[1].order]
            );
        }
        assert!(opening.iter().all(|s|s.iter().all(Option::is_some)),"{opening:?}");
        assert!(r.matter.economy.ledger.balanced());
        let restored =
            BlastRange::restore(r.config, &r.snapshot()).expect("battle construction restores");
        assert_eq!(r.state_hash(), restored.state_hash());
        let out=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../out/economy-543");
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(out.join("ai-opening.bin"),r.snapshot()).unwrap();
    }
    #[test]
    fn stock_commander_start_enemy_begins_building_without_player_autonomy() {
        let mut layout = RangeLayout::from_ron(include_str!("../../../data/range-km.ron")).unwrap();
        layout.battle = true; layout.endless_battle = false; layout.idle_enemies = false;
        layout.units.clear(); layout.unit_yaws.clear();
        if let Some(course) = &mut layout.course { course.patrols.clear(); }
        let y = layout.ground_level;
        let starts = [[layout.size[0] as i32 / 6, y, layout.size[2] as i32 / 6],
            [layout.size[0] as i32 * 5 / 6, y, layout.size[2] as i32 * 5 / 6]];
        for (side, at) in starts.into_iter().enumerate() {
            layout.blocks.push(RangeBlock { min: [(at[0]-850).max(1), y, (at[2]-850).max(1)],
                max: [(at[0]+850).min(layout.size[0] as i32 - 1), (y+220).min(layout.size[1] as i32), (at[2]+850).min(layout.size[2] as i32 - 1)], material: 0 });
            layout.units.push(RangeUnitDef { kind: RangeUnitKind::Commander, allied: side == 0, at, weapon: None });
        }
        let economy = modding::range_economy::RangeEconomy::stock();
        layout.economy = Some(economy);
        let stock=modding::range_economy::Balance::stock().initial_matter;
        layout.ready = Some(modding::range::RangeReady { commander_stock: stock,
            enemy_commander_stock: Some(stock), ..Default::default() });
        let config = modding::explosion::ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
        let mut r = BlastRange::with_layout(config, layout).unwrap();
        r.set_physics_workers(1); r.set_navigation_workers(1);
        r.start_battle().unwrap();
        r.economic_ai_phase();
        let sites = r.construction_sites();
        let origin = r.units[1].at;
        let validator = crate::blast::placement::Validator::default();
        let candidates: Vec<_> = [[300,0], [440,0], [-300,0], [0,300], [0,-300], [-440,0], [0,440], [0,-440]].into_iter().map(|[dx,dz]| {
            let x=origin[0]+dx; let z=origin[2]+dz; let at=[x,motion::floor(&r.world,x,z),z];
            (at, validator.runtime(&r, RangeUnitKind::KilFactoryT1, at, 2), validator.runtime_bounds(RangeUnitKind::KilFactoryT1, at, 2))
        }).collect();
        assert!(sites.iter().any(|s| r.can_commander_build(1, s.id)),
            "enemy failed: sites={sites:?}, position={:?}, bounds={:?}, order={:?}, held={:?}, candidates={candidates:?}", r.units[1].at, r.units[1].bounds(), r.battle.actors[1].order, r.tank_held(1));
        assert!(matches!(r.battle.actors[1].order, battle::Order::Build(_)), "enemy not building: {:?}", r.battle.actors[1].order);
        let site = sites.iter().find(|s| r.can_commander_build(1, s.id)).unwrap().id;
        for _ in 0..80 {
            r.construction_phase();
            r.tick += 1;
            if r.construction_sites()[site].delivered > 0 { break; }
        }
        assert!(r.construction_sites()[site].delivered > 0,
            "enemy site received no material: site={:?}, order={:?}, position={:?}", r.construction_sites()[site], r.battle.actors[1].order, r.units[1].at);
        assert!(r.matter.economy.ledger.balanced());
    }

}
