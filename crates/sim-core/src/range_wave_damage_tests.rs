use super::*;

struct Air;
impl Generator for Air {
    fn material(&self, _: u32, _: u32, _: u32) -> u16 {
        0
    }
}

fn config() -> ExplosionConfig {
    let mut c = ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
    c.energy = 100_000;
    c.half_cells = 16;
    c.cell_units = 1;
    c.air_cost = 1;
    c.unit_ablation_resistance = 1;
    c.unit_role_resistance_milli = [1000; 5];
    c.material_energy_j = 0;
    c
}

fn block() -> RangeUnit {
    let mut unit = RangeUnit::of_kind(RangeUnitKind::Tank, [64, 32, 64]);
    unit.body = UnitBody::new_scaled(10, 10, 10, 0, 100);
    for z in 0..10 {
        for y in 0..10 {
            for x in 0..10 {
                unit.body.set(x, y, z, voxel::Part::Armour);
            }
        }
    }
    unit
}

fn destroy(mut unit: RangeUnit, c: &ExplosionConfig, budget: usize) -> RangeUnit {
    let at = unit.at;
    let mut units = vec![unit];
    let mut world = World::new(128, 128, 128, 100, Box::new(Air));
    let mut dents = SurfaceWork::new();
    let mut explosion = Explosion::with_damage(&world, at, c, &units, &dents);
    while explosion.working() {
        explosion.step(
            &mut world,
            budget,
            &mut units,
            &mut dents,
            &mut Vec::new(),
            c,
            None,
        );
    }
    unit = units.pop().unwrap();
    unit
}

#[test]
fn overwhelming_wave_removes_rotated_armour_without_a_sampling_lattice() {
    for edge in [1, 2, 4] {
        for yaw in [
            0,
            fx::angle::FULL / 8,
            fx::angle::FULL / 7,
            -fx::angle::FULL / 11,
        ] {
            let mut c = config();
            c.cell_units = edge;
            let mut unit = block();
            unit.motion.drive.head = yaw;
            unit.update_pose(yaw);
            let unit = destroy(unit, &c, 4096);
            assert_eq!(
                unit.removed.len(),
                1000,
                "edge={edge}, yaw={yaw}: periodic survivors"
            );
        }
    }
}

#[test]
fn complete_tank_and_its_turned_plates_receive_the_overwhelming_wave() {
    let mut unit = RangeUnit::of_kind(RangeUnitKind::Tank, [64, 32, 64]);
    unit.motion.drive.head = fx::angle::FULL / 7;
    unit.update_pose(-fx::angle::FULL / 11);
    let root = unit.body.solid_count() as usize;
    let plates: usize = unit
        .body
        .segments
        .iter()
        .map(|s| s.body.solid_count() as usize)
        .sum();
    let mut c = config();
    c.cell_units = 2;
    let unit = destroy(unit, &c, 31);
    assert_eq!(unit.removed.len(), root);
    assert_eq!(unit.seg_removed.len(), plates);
}

#[test]
fn cell_ownership_is_unique_and_independent_of_live_damage() {
    let mut unit = block();
    unit.body.turret_y = 5;
    unit.motion.drive.head = fx::angle::FULL / 8;
    unit.update_pose(-fx::angle::FULL / 9);
    let query = HitFrames::new(&unit, unit.pose, &unit.seg_frames, unit.turret_delta);
    let mut contacts = Vec::new();
    let mut counts = BTreeMap::new();
    for z in 56..72 {
        for y in 30..44 {
            for x in 56..72 {
                query.wave_cells(
                    &unit,
                    &unit.pose,
                    &unit.seg_frames,
                    [x, y, z],
                    1,
                    &mut contacts,
                );
                for &cell in &contacts {
                    *counts.entry(cell).or_insert(0) += 1;
                }
            }
        }
    }
    assert_eq!(counts.len(), 1000);
    assert!(
        counts.values().all(|&n| n == 1),
        "a voxel took pressure more than once"
    );
    let before = counts;
    unit.receive_cell([5, 5, 5], 100_000, &config());
    let mut after = BTreeMap::new();
    for z in 56..72 {
        for y in 30..44 {
            for x in 56..72 {
                query.wave_cells(
                    &unit,
                    &unit.pose,
                    &unit.seg_frames,
                    [x, y, z],
                    1,
                    &mut contacts,
                );
                for &cell in &contacts {
                    *after.entry(cell).or_insert(0) += 1;
                }
            }
        }
    }
    assert_eq!(
        before, after,
        "removing a cell must not shift a deferred contact cursor"
    );
}

#[test]
fn captured_resistance_matches_an_exhaustive_voxel_centre_reference() {
    let mut unit = RangeUnit::of_kind(RangeUnitKind::Tank, [64, 32, 64]);
    unit.motion.drive.head = fx::angle::FULL / 7;
    unit.update_pose(-fx::angle::FULL / 11);
    unit.dents.insert([20, 10, 12], 1);
    for edge in [1, 2, 4] {
        let mut c = config();
        c.cell_units = edge;
        let world = World::new(128, 128, 128, 100, Box::new(Air));
        let e = Explosion::with_damage(
            &world,
            unit.at,
            &c,
            std::slice::from_ref(&unit),
            &SurfaceWork::new(),
        );
        let turret = crate::damage::turned(&unit.pose, unit.turret_delta, 0);
        let mut expected = BTreeMap::<[i32; 3], u32>::new();
        let mut reference = |part: usize, b: &UnitBody, root: bool| {
            for z in 0..b.vol.nz as usize {
                for y in 0..b.vol.ny as usize {
                    for x in 0..b.vol.nx as usize {
                        if !b.vol.get(x, y, z) {
                            continue;
                        }
                        let frame = if root {
                            if y < b.turret_y as usize {
                                &unit.pose
                            } else {
                                &turret
                            }
                        } else {
                            &unit.seg_frames[part - 1]
                        };
                        let p = frame.world([x, y, z].map(|v| Fx::from_ratio(2 * v as i32 + 1, 2)));
                        let cell = [p.x, p.y, p.z].map(|v| (v.raw() >> 32) as i32);
                        let wave = [0, 1, 2].map(|a| (cell[a] - e.origin[a]).div_euclid(edge));
                        let work = unit.hit_resistance(Some((part, [x, y, z])), &c);
                        let r = expected.entry(wave).or_default();
                        *r = (*r).max(work);
                    }
                }
            }
        };
        reference(0, &unit.body, true);
        for (i, seg) in unit.body.segments.iter().enumerate() {
            reference(i + 1, &seg.body, false);
        }
        let n = e.wave.size();
        for z in 0..n {
            for y in 0..n {
                for x in 0..n {
                    let cell = [x, y, z].map(|v| v as i32);
                    let half = n as i32 / 2;
                    let want = if cell.iter().map(|v| (v - half).pow(2)).sum::<i32>() > half * half
                    {
                        u32::MAX
                    } else {
                        expected.get(&cell).copied().unwrap_or(0)
                    };
                    assert_eq!(
                        e.wave.resistance_at([x, y, z]),
                        want,
                        "edge={edge}, cell={cell:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn tilted_and_subvoxel_bodies_have_no_missing_or_duplicate_owners() {
    for mm in [50, 100, 200] {
        for edge in [1, 2, 4] {
            let mut unit = block();
            unit.body.mm = mm;
            let frame = crate::damage::Frame::new(
                &unit.body,
                phys::V3::new(
                    Fx::from_ratio(6413, 100),
                    Fx::from_ratio(324, 10),
                    Fx::from_ratio(6487, 100),
                ),
                -fx::angle::FULL / 7,
                phys::V3::new(Fx::from_ratio(2, 7), Fx::ONE, Fx::from_ratio(-1, 3)),
            );
            let query = HitFrames::new(&unit, frame, &[], 0);
            let mut expected =
                BTreeMap::<[i32; 3], std::collections::BTreeSet<wave_cells::Contact>>::new();
            for z in 0..10 {
                for y in 0..10 {
                    for x in 0..10 {
                        let p = [x, y, z];
                        let w = frame.world(p.map(|v| Fx::from_ratio(2 * v as i32 + 1, 2)));
                        let low = [w.x, w.y, w.z]
                            .map(|v| ((v.raw() >> 32) as i32).div_euclid(edge) * edge);
                        expected.entry(low).or_default().insert((0, p));
                    }
                }
            }
            let mut contacts = Vec::new();
            for (low, want) in expected {
                query.wave_cells(&unit, &frame, &[], low, edge, &mut contacts);
                let got: std::collections::BTreeSet<_> = contacts.iter().copied().collect();
                assert_eq!(got.len(), contacts.len());
                assert_eq!(got, want, "mm={mm}, edge={edge}, low={low:?}");
            }
        }
    }
}

#[test]
fn pre_47_active_bursts_restore_with_their_legacy_contact_semantics() {
    let c = ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    let layout = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
    let mut range = BlastRange::with_layout(c.clone(), layout).unwrap();
    let mut explosion = Explosion::begin(range.units[0].at, &c, &range.units);
    explosion.point_contacts = true;
    range.explosion = Some(explosion);
    let mut bytes = range.snapshot();
    let mut extension = fx::blob::Blob::new();
    range
        .explosion
        .as_ref()
        .unwrap()
        .save_contacts(&mut extension);
    bytes.truncate(bytes.len() - extension.v.len());
    bytes[..4].copy_from_slice(&46u32.to_le_bytes());
    let restored = BlastRange::restore(c, &bytes).unwrap();
    assert!(restored.explosion.as_ref().unwrap().point_contacts);
    assert_eq!(restored.state_hash(), range.state_hash());
}

#[test]
#[ignore = "manual release timing; run alone"]
fn compare_capture_cost_with_legacy_point_sampling() {
    let mut unit = RangeUnit::of_kind(RangeUnitKind::Tank, [64, 32, 64]);
    unit.motion.drive.head = fx::angle::FULL / 7;
    unit.update_pose(-fx::angle::FULL / 11);
    let c = ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
    let world = World::new(128, 128, 128, 100, Box::new(Air));
    let dents = SurfaceWork::new();
    for legacy in [true, false] {
        let mut times = Vec::new();
        for _ in 0..21 {
            let mut e = Explosion::begin(unit.at, &c, std::slice::from_ref(&unit));
            e.point_contacts = legacy;
            let start = std::time::Instant::now();
            capture::capture(
                &mut e,
                &world,
                &c,
                std::slice::from_ref(&unit),
                &dents,
                usize::MAX,
                false,
            );
            times.push(start.elapsed().as_secs_f64() * 1000.0);
        }
        times.sort_by(f64::total_cmp);
        eprintln!("legacy={legacy}: capture median_ms={:.3}", times[10]);
    }
}
