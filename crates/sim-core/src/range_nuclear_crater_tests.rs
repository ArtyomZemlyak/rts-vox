use super::*;

#[test]
fn range_candidates_partial_tree_cache_never_aliases_another_row() {
    let terrain=RangeTerrain::new(false,None).with_tree_cache([56,128,56]);
    // A cache is only an optimization. Past its X extent, a flattened index
    // can still be in-bounds in another row; it must fall back by coordinates.
    let mut witnessed=0;
    for cz in 0..2 {for cx in 2..6 {
        let Some((base,tall,tx,tz))=terrain.tree_cell(cx,cz) else {continue};
        let lo=[tx-1,base,tz-1];let hi=[tx+2,base+tall,tz+2];
        let mats=terrain.material_candidates(lo,hi).unwrap();
        for y in base..base+tall {
            let mat=terrain.material(tx,y,tz);
            assert!(mats.contains(&mat),"cache alias cx={cx} cz={cz} at=[{tx},{y},{tz}] mat={mat} candidates={mats:?}");
            witnessed+=usize::from(mat==5);
        }
    }}
    assert!(witnessed>0);
}

struct Soil;

// Original pre-22 certificate, independent of the new cached wrapper.
fn pre22_confined(w:&World,p:[i32;3],edge:i32,pressure:u32,plane:i32,crater:Option<[i32;3]>,c:&ExplosionConfig)->bool {
    c.soil_depth_cost!=0 && pressure<=soil_box_confinement(p,edge,plane,crater,c)
        && p.iter().all(|&v|v>=0)
        && (w.box_is_material_or_air(p.map(|v|v as u32),p.map(|v|(v+edge) as u32),1)
            || w.material_candidates(p.map(|v|v as u32),p.map(|v|(v+edge) as u32))
                .is_some_and(|ids|ids.into_iter().all(|m|matches!(m,0|1|7|8))))
}

#[test]
fn wave_ceiling_cache_matches_uncached_clipping_and_live_edit_epochs() {
    struct Terrain;
    impl Generator for Terrain {
        fn material(&self,x:u32,y:u32,z:u32)->u16 {
            if y<16+(x/7+z/9)%12 {1}
            else if y==60 && x%11<3 && z%13<4 {3}else{0}
        }
        fn material_candidates(&self,_:[u32;3],_:[u32;3])->Option<Vec<u16>> {Some(vec![0,1,3])}
    }
    let mut w=World::new(96,96,96,65536,Box::new(Terrain));
    let c=ExplosionConfig::from_ron(include_str!("../../../data/nuclear-warhead.ron")).unwrap();
    for epoch in 0..2 {
        if epoch==1 {w.queue_edit(voxel::Edit::put(70,95,70,65535));w.queue_edit(voxel::Edit::dig(8,60,13));w.flush();}
        for edge in [1,4,16,32,64] {
            let n=4;let origin=[-edge,-edge,-edge];let mut cache=vec![i32::MIN;n*n];
            for cz in 0..n {for cx in 0..n {for cy in -1..7 {
                let cell=[cx as i32,cy,cz as i32];let p=std::array::from_fn(|a|origin[a]+cell[a]*edge);
                assert_eq!(cell_is_air_cached(&w,p,edge,cell,n,&mut cache),cell_is_air(&w,p,edge),"epoch={epoch} p={p:?} edge={edge}");
                assert_eq!(cell_has_no_solid_cached(&w,p,edge,cell,n,&mut cache),cell_has_no_solid(&w,p,edge));
                // Certificate strength/classification may improve; both must
                // certify only genuinely silent soil (or an empty box).
                let old=pre22_confined(&w,p,edge,0,24,Some([48,24,48]),&c);
                let new=confined_soil_box_cached(&w,p,edge,cell,n,&mut cache,0,24,Some([48,24,48]),&c);
                if old||new {
                    for z in p[2].max(0)..(p[2]+edge).min(96) {for y in p[1].max(0)..(p[1]+edge).min(96) {for x in p[0].max(0)..(p[0]+edge).min(96) {
                        if w.solid(x as u32,y as u32,z as u32) {
                            let m=w.material(x as u32,y as u32,z as u32);assert!(matches!(m,1|7|8),"false confined certificate m={m}");
                        }
                    }}}
                }
            }}}
            let p=[24,12,24];let mut empty=[];
            assert_eq!(cell_is_air_cached(&w,p,edge,[-1;3],0,&mut empty),cell_is_air(&w,p,edge));
        }
    }
}

#[test]
fn crater_planes_match_direct_voxels_for_odd_edges_clipped_world_and_unknown_materials() {
    struct Mixed;
    impl Generator for Mixed {
        fn material(&self,x:u32,y:u32,z:u32)->u16 {
            if y<12 {1}else if y==12 {if (x+z)%2==0 {7}else{8}}
            else if y<29 {(x*3+z*7+y).is_multiple_of(5).then_some([4,5,6][((x+z+y)%3) as usize]).unwrap_or(0)}else{0}
        }
        fn batches_columns(&self)->bool {true}
        fn material_candidates(&self,_:[u32;3],_:[u32;3])->Option<Vec<u16>> {Some(vec![0,1,4,5,6,7,8])}
        fn uniform_box(&self,lo:[u32;3],hi:[u32;3])->Option<u16> {if hi[1]<=12 {Some(1)}else if lo[1]>=29 {Some(0)}else{None}}
    }
    let mut w=World::new(96,96,96,65536,Box::new(Mixed));
    for (p,m) in [([0,12,0],65535),([15,8,15],3),([32,12,32],2),([95,18,95],65535),([63,11,31],0)] {w.queue_edit(voxel::Edit::put(p[0],p[1],p[2],m));}
    w.flush();let config=ExplosionConfig::from_ron(include_str!("../../../data/nuclear-warhead.ron")).unwrap();
    for terraced in [false,true] {
        let mut c=config;if !terraced {c.soil_crater=None;}
        for (edge,p) in [(2,[-1,11,-1]),(3,[15,8,15]),(4,[31,11,31]),(5,[92,8,92]),(15,[-2,-2,-2]),(16,[0,0,0]),(17,[24,0,24]),(31,[32,4,32]),(32,[-8,-8,-8]),(63,[64,0,64]),(64,[-8,-8,-8]),(64,[95,0,95])] {
            for pressure in [0,283,284,2400] {
                let mut payload=vec![77];let mask=read_crater_cell_into(&w,p,edge,pressure,12,[48,12,48],&c,&mut payload).unwrap();
                let mut expected=vec![77];let mut subs=Vec::new();
                for i in 0..(edge as usize).pow(3) {
                    let e=edge as usize;let q=[p[0]+(i%e) as i32,p[1]+((i/e)%e) as i32,p[2]+(i/(e*e)) as i32];
                    if q.iter().any(|&v|!(0..96).contains(&v)) || !w.solid(q[0] as u32,q[1] as u32,q[2] as u32) {continue;}
                    let m=w.material(q[0] as u32,q[1] as u32,q[2] as u32);
                    if matches!(m,1|7|8) && soil_contact_pressure(pressure,soil_load(q,12,Some([48,12,48]),&c),m,true,&c)==0 {continue;}
                    subs.push(i);expected.push(m);
                }
                assert_eq!(mask.ranked_from(0).map(|(_,s)|s).collect::<Vec<_>>(),subs,"edge={edge} p={p:?} pressure={pressure} terraced={terraced}");
                assert_eq!(payload,expected,"payload order edge={edge} p={p:?} pressure={pressure}");
            }
        }
        for edge in [1,65] {
            let mut payload=vec![77];let mut reference=vec![77];
            let old=read_terraced_soil(&w,[0;3],edge,2400,12,[48,12,48],&c,&mut reference);
            assert_eq!(read_crater_cell_into(&w,[0;3],edge,2400,12,[48,12,48],&c,&mut payload),old);
            assert_eq!(payload,reference,"existing edge={edge} fallback must remain unchanged");
        }
        c.soil_depth_cost=0;let mut payload=vec![77];assert!(read_crater_cell_into(&w,[0;3],16,2400,12,[48,12,48],&c,&mut payload).is_none());assert_eq!(payload,[77]);
    }
}

#[test]
fn range_candidates_cover_default_structures_surfaces_and_authored_overrides() {
    let mut layout=RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
    layout.ground_level=80;layout.size=[512,192,512];
    layout.blocks.push(modding::range::RangeBlock{min:[32,70,32],max:[48,90,48],material:65535});
    layout.blocks.push(modding::range::RangeBlock{min:[36,74,36],max:[40,80,40],material:0});
    for (extended,authored) in [(false,false),(true,false),(false,true)] {
        for cached in [false,true] {
            let mut t=RangeTerrain::new(extended,authored.then(||layout.clone()));
            if cached {t=t.with_tree_cache([56,192,56]);}
            for x in [27,28,31,32,36,39,47,48,231,232,249,250,255,256,263,264,391,392,409,410,416,423] {
                for z in [27,31,32,39,47,48,290,291,320,321,350,351,382,383] {
                    for y in [0,14,15,16,17,44,45,65,66,69,70,73,74,79,80,81,89,90] {
                        let lo=[x,y,z];let hi=[x+2,y+2,z+2];let ids=t.material_candidates(lo,hi).unwrap();
                        for dz in 0..2 {for dy in 0..2 {for dx in 0..2 {
                            let q=[x+dx,y+dy,z+dz];let m=t.material(q[0],q[1],q[2]);
                            assert!(ids.contains(&m),"missing {m} at {q:?}:extended={extended} authored={authored} cached={cached} candidates={ids:?}");
                        }}}
                    }
                }
            }
        }
    }
}
impl Generator for Soil {
    fn material(&self, _: u32, y: u32, _: u32) -> u16 {
        u16::from(y < 80)
    }
    fn batches_columns(&self) -> bool {
        true
    }
    fn column(&self, _: u32, _: u32, y: u32, out: &mut [u16]) {
        for (dy, m) in out.iter_mut().enumerate() {
            *m = u16::from(y + (dy as u32) < 80);
        }
    }
    fn material_candidates(&self, _: [u32; 3], _: [u32; 3]) -> Option<Vec<u16>> {
        Some(vec![0, 1])
    }
    fn uniform_box(&self, lo: [u32; 3], hi: [u32; 3]) -> Option<u16> {
        if hi[1] <= 80 {
            Some(1)
        } else if lo[1] >= 80 {
            Some(0)
        } else {
            None
        }
    }
}

#[test]
fn nuclear_crater_is_wide_cubic_and_filtered_contacts_match_scalar_soil() {
    let c = ExplosionConfig::from_ron(include_str!("../../../data/nuclear-warhead.ron")).unwrap();
    let at = [1024, 79, 512];
    // A fixed 48 m control belonged to the former 2400-unit charge. Bound
    // the control from the authored lateral load: even full source pressure
    // cannot excavate this terrace, regardless of air-path attenuation.
    let shape=c.soil_crater.unwrap();
    let far=((u64::from(c.energy)*u64::from(shape.spread)/u64::from(c.soil_depth_cost)
        /u64::from(shape.step)+2)*u64::from(shape.step)) as usize;
    let centre=at[0] as usize;
    assert!(soil_load([at[0]+far as i32,at[1],at[2]],at[1],Some(at),&c)>=c.energy);
    let mut profiles = Vec::new();
    for filtered in [false, true] {
        let mut w = World::new(2048, 128, 1024, 65536, Box::new(Soil));
        // Non-soil material outside the crater still meets full blast pressure.
        for (x, mat) in [(2030, 2), (2031, 4), (2032, 5), (2033, 3)] {
            w.queue_edit(voxel::Edit::put(x, 79, 512, mat));
        }
        w.flush();
        let mut dents = SurfaceWork::new();
        let mut e = Explosion::with_damage(&w, at, &c, &[], &dents);
        e.nuclear_ground = true;
        let m = matter::Matter::new(&[]);
        let mut ticks = 0;
        while (e.working() || w.queued() > 0) && ticks < 2048 {
            e.advance_frontier(c.wave_work_per_tick, &w, &[]);
            if filtered {
                e.read_contacts_with(&w, c.wave_work_per_tick, &[], Some((&m, &c, u64::MAX)));
            }
            let before = w.queued();
            e.step_with_room::<true>(
                &mut w,
                c.wave_work_per_tick,
                &mut [],
                &mut dents,
                &mut Vec::new(),
                &c,
                None,
                None,
            );
            assert!(
                w.queued() - before <= c.wave_work_per_tick,
                "ground exceeded its slice"
            );
            w.flush();
            ticks += 1;
        }
        assert!(
            !e.working() && w.queued() == 0,
            "unfinished after {ticks} ticks"
        );
        let depth_at = |x, z| {
            (0..80)
                .rev()
                .find(|&y| w.material(x, y, z) != 0)
                .map_or(80, |y| 79 - y)
        };
        let profile: Vec<_> = (0..2048).map(|x| depth_at(x, 512)).collect();
        assert!(profile[centre] >= 48, "crater too shallow: {}", profile[centre]);
        assert!(profile[centre] > profile[centre + 192]);
        assert!(profile[centre + 320] > 0, "crater did not widen");
        assert!(depth_at(at[0] as u32 + 288, 512 + 288) > 0, "lost square corners");
        assert_eq!(profile[centre + far], 0, "crater stripped soil beyond its full-pressure bound");
        assert!(dents.is_empty(), "strategic soil created per-voxel dents");
        for x in 2030..2034 {
            assert_eq!(w.material(x, 79, 512), 0);
        }
        println!(
            "filtered={filtered} ticks={ticks} centre_depth={} width={}",
            profile[centre],
            profile.iter().filter(|&&d| d > 0).count()
        );
        profiles.push((profile, dents));
    }
    assert_eq!(
        profiles[0], profiles[1],
        "silent-soil filtering changed damage"
    );
}

#[test]
fn nuclear_crater_box_proofs_preserve_mixed_material_and_live_edits() {
    let c = ExplosionConfig::from_ron(include_str!("../../../data/nuclear-warhead.ron")).unwrap();
    let at = [512, 79, 512];
    let mut w = World::new(768, 128, 768, 65536, Box::new(Soil));
    let p = [0, 0, 512];
    assert!(confined_soil_box(&w, p, 64, 2400, 79, Some(at), &c));
    for mat in [2, 3, 4, 5, 6, 65535] {
        w.queue_edit(voxel::Edit::put(20, 40, 530, mat));
        w.flush();
        assert!(!confined_soil_box(&w, p, 64, 2400, 79, Some(at), &c));
    }
    assert_eq!(
        soil_lateral_work([i32::MAX, 0, i32::MIN], Some(at), &c),
        u32::MAX
    );
    assert_eq!(soil_lateral_work(p, None, &c), 0);
}

#[test]
fn nuclear_crater_zero_depth_cost_preserves_partial_soil_contacts() {
    let mut c =
        ExplosionConfig::from_ron(include_str!("../../../data/nuclear-warhead.ron")).unwrap();
    c.soil_depth_cost = 0;
    c.cell_units = 4;
    c.half_cells = 2;
    assert!(c.validate().is_ok());
    let pressure = material_work(1, &c) - 1;
    let mut w = World::new(64, 32, 64, 65536, Box::new(Soil));
    let mut dents = SurfaceWork::new();
    let mut e = Explosion::begin([24, 15, 24], &c, &[]);
    e.nuclear_ground = true;
    e.origin = [16, 8, 16];
    e.pending.push_back(phys::blast::Hit {
        cell: [0; 3],
        pressure,
        broken: false,
    });
    let m = matter::Matter::new(&[]);
    e.read_contacts_with(&w, 65536, &[], Some((&m, &c, u64::MAX)));
    assert_eq!(e.solid.len(), 1);
    assert_eq!(
        e.solid[0].1.count_ones(),
        64,
        "disabled confinement must keep weak contacts"
    );
    assert_eq!(e.solid_mats, vec![1; 64]);
    assert!(e.ground_contact_known(
        &mut w,
        [16, 8, 16],
        1,
        pressure,
        &mut dents,
        &mut Vec::new(),
        &c,
        None
    ));
    assert_eq!(dents.get(&[16, 8, 16]), Some(&pressure));
    assert_eq!(w.queued(), 0);
}

#[test]
fn nuclear_crater_row_reader_matches_scalar_with_holes_edges_and_soil_materials() {
    struct Layer(u16);
    impl Generator for Layer {
        fn material(&self, _: u32, y: u32, _: u32) -> u16 {
            if y < 80 {
                self.0
            } else {
                0
            }
        }
        fn material_candidates(&self, _: [u32; 3], _: [u32; 3]) -> Option<Vec<u16>> {
            Some(vec![0, self.0])
        }
    }
    let c = ExplosionConfig::from_ron(include_str!("../../../data/nuclear-warhead.ron")).unwrap();
    let at = [40, 79, 40];
    for mat in [1, 7, 8] {
        let mut w = World::new(128, 128, 128, 65536, Box::new(Layer(mat)));
        for p in [[1, 70, 12], [45, 63, 40], [50, 79, 50]] {
            w.queue_edit(voxel::Edit::dig(p[0], p[1], p[2]));
        }
        w.flush();
        for edge in [3, 16, 64] {
            for p in [[-8, 64, -8], [32, 32, 32], [100, 64, 100], [20, 79, 20]] {
                for pressure in [0, 283, 284, 1500, 2400] {
                    let mut actual = vec![42];
                    let fast =
                        read_terraced_soil(&w, p, edge, pressure, 79, at, &c, &mut actual).unwrap();
                    let offsets = subcell_offsets(edge);
                    let mut raw = Vec::new();
                    let mask = read_cell_into(&w, p, edge, [0; 3], &offsets, &mut raw);
                    let mut expected_mask = contact_mask::Mask::for_voxels((edge as usize).pow(3));
                    let mut expected = vec![42];
                    for (rank, sub) in mask.ranked_from(0) {
                        let q = std::array::from_fn(|a| p[a] + offsets[sub][a]);
                        if soil_contact_pressure(
                            pressure,
                            soil_load(q, 79, Some(at), &c),
                            raw[rank],
                            true,
                            &c,
                        ) > 0
                        {
                            expected_mask.insert(sub);
                            expected.push(raw[rank]);
                        }
                    }
                    assert_eq!(
                        fast, expected_mask,
                        "mat={mat} edge={edge} p={p:?} pressure={pressure}"
                    );
                    assert_eq!(actual, expected);
                }
            }
        }
        w.queue_edit(voxel::Edit::put(40, 75, 40, 65535));
        w.flush();
        let mut payload = vec![42];
        assert!(read_terraced_soil(&w, [32, 64, 32], 16, 2400, 79, at, &c, &mut payload).is_none());
        assert_eq!(payload, [42], "fallback must not append a partial payload");
    }
}

#[test]
fn nuclear_crater_mixed_forest_cell_reader_and_range_terrain_candidates_match_scalar() {
    let c = ExplosionConfig::from_ron(include_str!("../../../data/nuclear-warhead.ron")).unwrap();
    let mut layout =
        RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
    layout.size = [512, 128, 512];
    layout.clear_min = [32, 32];
    layout.clear_max = [96, 96];
    let terrain = RangeTerrain::new(false, Some(layout)).with_tree_cache([512, 128, 512]);
    for z0 in (0..512u32).step_by(64) {
        for x0 in (0..512u32).step_by(64) {
            for (y0, y1) in [(0u32, 14u32), (14, 48), (48, 96), (0, 128)] {
                let cands = terrain
                    .material_candidates([x0, y0, z0], [x0 + 64, y1, z0 + 64])
                    .expect("bounded range candidates");
                for z in (z0..z0 + 64).step_by(7) {
                    for x in (x0..x0 + 64).step_by(7) {
                        for y in [y0, (y0 + y1) / 2, y1 - 1] {
                            let mat = terrain.material(x, y, z);
                            assert!(
                                cands.contains(&mat),
                                "missing mat={mat} at [{x},{y},{z}] in [{x0}..{},{y0}..{y1},{z0}..{}]: {cands:?}",
                                x0 + 64,
                                z0 + 64
                            );
                        }
                    }
                }
            }
        }
    }

    let mut w = World::new(512, 128, 512, 65536, Box::new(terrain));
    w.queue_edit(voxel::Edit::put(260, 22, 260, 2));
    w.queue_edit(voxel::Edit::put(261, 22, 260, 3));
    w.queue_edit(voxel::Edit::dig(262, 10, 262));
    w.flush();
    let at = [256, 16, 256];
    let plane = soil_reference(&w, at);
    for edge in [4, 16, 64] {
        let offsets = subcell_offsets(edge);
        for p in [[-8, 0, 240], [192, 0, 192], [240, 8, 240], [256, 16, 256], [320, 0, 320]] {
            for pressure in [0, 300, 900, 1800, 3000] {
                let mut actual = vec![99];
                let mask_opt =
                    read_crater_cell_into(&w, p, edge, pressure, plane, at, &c, &mut actual);
                let fast = mask_opt.expect("crater reader handles 2..=64 world cells");
                let mut raw = Vec::new();
                let raw_mask = read_cell_into(&w, p, edge, [0; 3], &offsets, &mut raw);
                let mut expected_mask = contact_mask::Mask::for_voxels((edge as usize).pow(3));
                let mut expected = vec![99];
                for (rank, sub) in raw_mask.ranked_from(0) {
                    let mat = raw[rank];
                    let q = std::array::from_fn(|a| p[a] + offsets[sub][a]);
                    let silent = matches!(mat, 1 | 7 | 8)
                        && soil_contact_pressure(
                            pressure,
                            soil_load(q, plane, Some(at), &c),
                            mat,
                            true,
                            &c,
                        ) == 0;
                    if !silent {
                        expected_mask.insert(sub);
                        expected.push(mat);
                    }
                }
                assert_eq!(
                    fast.count_ones(),
                    expected_mask.count_ones(),
                    "edge={edge} p={p:?} pressure={pressure}"
                );
                let fast_subs: Vec<_> = fast.ranked_from(0).map(|(_, s)| s).collect();
                let exp_subs: Vec<_> = expected_mask.ranked_from(0).map(|(_, s)| s).collect();
                assert_eq!(fast_subs, exp_subs, "edge={edge} p={p:?} pressure={pressure}");
                assert_eq!(actual, expected, "edge={edge} p={p:?} pressure={pressure}");
            }
        }
    }

    let mut e = Explosion::with_damage(&w, at, &c, &[], &SurfaceWork::new());
    e.nuclear_ground = true;
    let mut e_ref = Explosion::with_damage(&w, at, &c, &[], &SurfaceWork::new());
    e_ref.nuclear_ground = true;
    e.advance_frontier(65_536, &w, &[]);
    let hits = e_ref.wave.advance_contacts(65_536);
    let expected_pending: Vec<_> = hits
        .into_iter()
        .filter(|hit| {
            let p = std::array::from_fn(|a| e_ref.origin[a] + hit.cell[a] * e_ref.cell);
            !cell_is_air(&w, p, e_ref.cell)
        })
        .collect();
    let actual_pending: Vec<_> = e.pending.iter().copied().collect();
    assert_eq!(actual_pending, expected_pending);
}
