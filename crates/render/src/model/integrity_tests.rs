//! Audit the render geometry independently of the physical-volume cavity tests.
use super::*;
use voxel::unit::Part;

fn templates() -> Vec<(&'static str, UnitBody)> {
    let mut bodies = voxel::roster::all();
    bodies.extend([
        ("range walker", voxel::unit::strider()),
        ("range walker04", voxel::unit::walker04()),
        ("range crawler", voxel::unit::crawler()),
        ("range razlom", voxel::unit::razlom_t3()),
        ("greben storage", voxel::unit::greben_storage()),
        ("range spider", voxel::unit::spider()),
        ("range artillery", voxel::unit::artillery()),
        ("range artillery T2", voxel::unit::artillery_t2()),
        (
            "range artillery turret T2",
            voxel::unit::artillery_turret_t2(),
        ),
        (
            "range artillery turret T3",
            voxel::unit::artillery_turret_t3(),
        ),
        ("range turret T1", voxel::unit::turret_t1()),
        ("range turret T2", voxel::unit::turret_t2()),
        ("range command base", voxel::unit::command_base()),
        ("biped", voxel::unit::biped()),
    ]);
    bodies
}

#[test]
fn crawler_drill_is_rendered_with_the_hull_not_as_a_turret() {
    let body = voxel::unit::crawler();
    let cut = body.turret_y as usize;
    assert_eq!(cut, body.vol.ny as usize);
    let hull = mesh(&body, (0, cut), BEVEL);
    let turret = mesh(&body, (cut, usize::MAX), BEVEL);
    assert!(hull.iter().any(|part| !part.idx.is_empty()));
    assert!(turret.iter().all(|part| part.idx.is_empty()));
}

fn mesh(body: &UnitBody, rows: (usize, usize), bevel: f32) -> [MeshBuf; crate::look::FINISHES] {
    let mut out = std::array::from_fn(|_| MeshBuf::default());
    body_to_mesh(body, [0.3, 0.5, 0.9], 1.0, rows, bevel, [0.0; 3], &mut out);
    out
}

/// Removing the other moving part before meshing is an independent oracle:
/// rotating it away must never expose faces culled against its old location.
#[test]
fn every_stock_hull_and_turret_matches_an_isolated_physical_part() {
    let models = templates();
    let mut checked = 0;
    for (name, body) in &models {
        if body.turret_y == 0 {
            continue;
        }
        let cut = body.turret_y as usize;
        for rows in [(0, cut), (cut, body.vol.ny as usize)] {
            let mut isolated = body.clone();
            for z in 0..body.vol.nz as usize {
                for y in 0..body.vol.ny as usize {
                    for x in 0..body.vol.nx as usize {
                        if y < rows.0 || y >= rows.1 {
                            isolated.remove(x, y, z);
                        }
                    }
                }
            }
            for bevel in [0.0, 0.35, BEVEL] {
                let actual = mesh(body, rows, bevel);
                let expected = mesh(&isolated, (0, usize::MAX), bevel);
                for (finish, (a, e)) in actual.iter().zip(&expected).enumerate() {
                    assert!(a.pos == e.pos, "{name} rows={rows:?} bevel={bevel} finish={finish}: exposed moving interface is missing geometry ({} vs {} vertices)", a.pos.len(), e.pos.len());
                    assert_eq!(a.nrm, e.nrm, "{name}: interface normals");
                    assert_eq!(a.idx, e.idx, "{name}: interface indices");
                }
            }
            checked += 1;
        }
    }
    eprintln!(
        "audited {checked} independent hull/turret meshes across {} models",
        models.len()
    );
}

#[test]
fn all_stock_volumes_emit_each_exterior_face_with_outward_winding() {
    let mut volumes = 0;
    for (name, root) in templates() {
        for (part, body) in std::iter::once((None, &root)).chain(
            root.segments
                .iter()
                .enumerate()
                .map(|(i, s)| (Some(i), &s.body)),
        ) {
            let size = [body.vol.nx as i32, body.vol.ny as i32, body.vol.nz as i32];
            let mut faces = 0;
            for z in 0..size[2] {
                for y in 0..size[1] {
                    for x in 0..size[0] {
                        if !body.vol.get(x as usize, y as usize, z as usize) {
                            continue;
                        }
                        for axis in 0..3 {
                            for sign in [-1, 1] {
                                let mut q = [x, y, z];
                                q[axis] += sign;
                                if (0..3).any(|k| q[k] < 0 || q[k] >= size[k])
                                    || !body.vol.get(q[0] as usize, q[1] as usize, q[2] as usize)
                                {
                                    faces += 1;
                                }
                            }
                        }
                    }
                }
            }
            for bevel in [0.0, 0.35, BEVEL] {
                let buffers = mesh(body, (0, usize::MAX), bevel);
                if bevel == 0.0 {
                    assert_eq!(
                        buffers.iter().map(|m| m.idx.len()).sum::<usize>(),
                        faces * 6,
                        "{name} part={part:?}: missing exterior faces"
                    );
                }
                for m in &buffers {
                    for t in m.idx.chunks_exact(3) {
                        let [a, b, c] = [
                            m.pos[t[0] as usize],
                            m.pos[t[1] as usize],
                            m.pos[t[2] as usize],
                        ]
                        .map(Vec3::from);
                        let n = Vec3::from(m.nrm[t[0] as usize]);
                        assert!(
                            (b - a).cross(c - a).dot(n) > 0.0,
                            "{name} part={part:?} bevel={bevel}: reversed or degenerate triangle"
                        );
                    }
                }
            }
            volumes += 1;
        }
    }
    eprintln!("audited exterior faces and winding of {volumes} physical volumes");
    assert!(volumes >= 367);
}

/// A ray intersects actual triangles, so this checks outward winding as well
/// as coverage. A small non-centred sample avoids triangle-edge ambiguity.
fn hits_y(meshes: &[MeshBuf], x: f32, z: f32, height: f32, normal: f32) -> bool {
    meshes.iter().any(|m| {
        m.idx.chunks_exact(3).any(|t| {
            let [a, b, c] = [
                m.pos[t[0] as usize],
                m.pos[t[1] as usize],
                m.pos[t[2] as usize],
            ];
            if [a, b, c].iter().any(|p| (p[1] - height).abs() > 1e-5) {
                return false;
            }
            let a = Vec3::from(a);
            let b = Vec3::from(b);
            let c = Vec3::from(c);
            if (b - a).cross(c - a).y * normal <= 0.0 {
                return false;
            }
            let p = Vec3::new(x, height, z);
            let edges = [(a, b), (b, c), (c, a)].map(|(a, b)| (b - a).cross(p - a).y * normal);
            edges.iter().all(|s| *s >= -1e-5)
        })
    })
}

#[test]
fn spider_deck_and_laser_underside_remain_closed_when_head_turns() {
    let spider = voxel::unit::spider();
    let cut = spider.turret_y as usize;
    let hull = mesh(&spider, (0, cut), BEVEL);
    let head = mesh(&spider, (cut, usize::MAX), BEVEL);
    let mut contact = 0;
    for z in 0..spider.vol.nz as usize {
        for x in 0..spider.vol.nx as usize {
            if spider.vol.get(x, cut - 1, z) && spider.vol.get(x, cut, z) {
                let (px, pz) = (x as f32 + 0.43, z as f32 + 0.57);
                // At the lip the bevel intentionally trims the outer half-cell.
                // Interior deck samples cannot be removed by rounding.
                if x > 0
                    && z > 0
                    && [(-1, 0), (1, 0), (0, -1), (0, 1)].iter().all(|&(dx, dz)| {
                        spider
                            .vol
                            .get((x as i32 + dx) as usize, cut - 1, (z as i32 + dz) as usize)
                            && spider.vol.get(
                                (x as i32 + dx) as usize,
                                cut,
                                (z as i32 + dz) as usize,
                            )
                    })
                {
                    assert!(
                        hits_y(&hull, px, pz, cut as f32, 1.0),
                        "open deck x={x} z={z}"
                    );
                    assert!(
                        hits_y(&head, px, pz, cut as f32, -1.0),
                        "open underside x={x} z={z}"
                    );
                    contact += 1;
                }
            }
        }
    }
    assert!(
        contact > 500,
        "spider interface audit is too small: {contact}"
    );
}

#[test]
fn capped_moving_parts_preserve_real_through_damage() {
    let mut body = UnitBody::new(7, 6, 7, 1);
    for z in 0..7 {
        for y in 0..6 {
            for x in 0..7 {
                body.set(x, y, z, Part::Armour);
            }
        }
    }
    body.turret_y = 3;
    for y in 0..6 {
        body.remove(3, y, 3);
    }
    for (rows, direction) in [((0, 3), 1.0), ((3, 6), -1.0)] {
        let part = mesh(&body, rows, BEVEL);
        assert!(
            hits_y(&part, 1.43, 1.57, 3.0, direction),
            "surviving armour lacks a cap"
        );
        assert!(
            !hits_y(&part, 3.43, 3.57, 3.0, direction),
            "real damage was covered"
        );
    }
}
