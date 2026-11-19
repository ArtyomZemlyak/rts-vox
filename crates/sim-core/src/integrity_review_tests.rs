use super::*;

fn bored(rects: &[(usize, usize, usize, usize)]) -> (Status, Report) {
    let mut body = voxel::unit::commander();
    let model = Model::new(&body);
    let origin = [body.core.0, body.core.1, body.core.2].map(usize::from);
    assert_eq!(body.core_size, 8);
    for &(y0, y1, z0, z1) in rects {
        for x in 0..8 {
            for y in y0..y1 {
                for z in z0..z1 {
                    body.remove(origin[0] + x, origin[1] + y, origin[2] + z);
                }
            }
        }
    }
    model.evaluate(&body, &mut CoreBufs::new())
}

#[test]
fn independent_small_disconnected_tunnels_do_not_sum_to_one_catastrophic_cavity() {
    // Two separated 6/64 tunnels. Neither connected cavity reaches 15%.
    let (status, report) = bored(&[(1, 3, 1, 4), (5, 7, 4, 7)]);
    assert!(report.core >= Fx::from_ratio(3, 4));
    assert!(status.alive, "separate small bores must not be summed into one breach");
}

#[test]
fn independent_connected_tunnel_above_threshold_is_lethal_without_mass_loss_threshold() {
    let (status, report) = bored(&[(1, 4, 1, 5)]);
    assert!(report.core >= Fx::from_ratio(3, 4));
    assert!(!status.alive, "a connected 12/64 through-cavity must destroy the core");
}
