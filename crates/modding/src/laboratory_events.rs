//! Placeable laboratory inputs. These presets configure the shared weapon;
//! they do not implement an alternative blast, thermal or support solver.
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub struct Event {
    pub name: String,
    pub description: String,
    pub icon: String,
    pub effect: u8,
    pub power_milli: u32,
    pub detonate: bool,
}
pub fn stock() -> Vec<Event> {
    let events: Vec<Event> =
        ron::from_str(include_str!("../../../data/lab-events.ron")).expect("laboratory events RON");
    for e in &events {
        assert!(!e.name.is_empty() && e.name.chars().count() <= 64);
        assert!(!e.description.is_empty() && e.description.len() <= 1024);
        assert!(e.effect <= 9 && (100..=100000).contains(&e.power_milli));
        assert!(!e.detonate || matches!(e.effect, 0 | 2 | 5 | 6 | 8 | 9));
        assert!([
            "bomb",
            "radiation",
            "hammer",
            "zap",
            "flame",
            "sparkles",
            "crosshair"
        ]
        .contains(&e.icon.as_str()));
    }
    events
}
