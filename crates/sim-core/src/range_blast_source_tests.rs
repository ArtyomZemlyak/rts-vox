use super::*;
use fx::blob::Blob;
use voxel::Generator;

struct Face {
    axis: usize,
    incoming: i32,
    narrow_gap: bool,
}
impl Generator for Face {
    fn material(&self, x: u32, y: u32, z: u32) -> u16 {
        let q = [x, y, z][self.axis] as i32;
        if (q - 64) * self.incoming <= 0 || (self.narrow_gap && q <= 61) {
            3
        } else {
            0
        }
    }
}
fn config(edge: i32) -> ExplosionConfig {
    let mut c = ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    c.cell_units = edge;
    c
}

#[test]
fn every_surface_direction_and_cell_scale_emits_only_on_the_open_side() {
    for edge in 1..=4 {
        let c = config(edge);
        for axis in 0..3 {
            for sign in [-1, 1] {
                let w = World::new(
                    128,
                    128,
                    128,
                    100,
                    Box::new(Face {
                        axis,
                        incoming: sign,
                        narrow_gap: false,
                    }),
                );
                let at = [64; 3];
                let mut from = at;
                from[axis] += sign;
                let mut e = Explosion::begin_impact(&w, at, from, &c, &[], 40);
                assert!(e.capture(&w, &c, &[], &Default::default(), usize::MAX));
                let centre = [e.wave.size() / 2; 3];
                assert_eq!(e.wave.resistance_at(centre), c.steel_resistance);
                while !e.wave.finished() {
                    e.wave.advance_contacts(31);
                }
                let mut air = centre;
                air[axis] = (air[axis] as i32 + sign) as usize;
                let mut behind = centre;
                behind[axis] = (behind[axis] as i32 - sign) as usize;
                assert_eq!(
                    e.wave.pressure_at(air),
                    c.energy - 100,
                    "edge={edge}, axis={axis}, sign={sign}"
                );
                assert_eq!(
                    e.wave.pressure_at(behind),
                    0,
                    "pressure crossed intact steel"
                );
            }
        }
    }
}

#[test]
fn a_narrow_air_gap_does_not_seed_through_matter_and_airbursts_keep_their_source() {
    let c = config(4);
    let w = World::new(
        128,
        128,
        128,
        100,
        Box::new(Face {
            axis: 0,
            incoming: -1,
            narrow_gap: true,
        }),
    );
    for (at, from) in [([64; 3], [63, 64, 64]), ([63, 64, 64], [62, 64, 64])] {
        let actual = Explosion::begin_impact(&w, at, from, &c, &[], 40);
        let expected = Explosion::begin_material(at, &c, &[], 40);
        let mut a = Blob::new();
        let mut b = Blob::new();
        actual.save(&mut a);
        expected.save(&mut b);
        assert_eq!(
            a.v, b.v,
            "buried detonation/airburst changed its initial source"
        );
    }
}
