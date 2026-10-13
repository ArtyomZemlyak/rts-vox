//! Schedule boundaries only; no new ordering between Bevy's parallel systems.
use super::{FrameSource, Timeline};
use bevy::{app::MainScheduleOrder, ecs::schedule::ScheduleLabel, prelude::*};

#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
enum Boundary {
    Before,
    BeforeEnd,
    After,
    AfterEnd,
}
#[derive(Resource, Default)]
struct Tokens {
    before: Option<usize>,
    after: Option<usize>,
}
pub(super) fn install(app: &mut App) {
    app.init_resource::<Tokens>()
        .init_resource::<MainScheduleOrder>()
        .add_systems(Boundary::Before, before)
        .add_systems(Boundary::BeforeEnd, before_end)
        .add_systems(Boundary::After, after)
        .add_systems(Boundary::AfterEnd, after_end);
    let mut order = app.world_mut().resource_mut::<MainScheduleOrder>();
    order.insert_before(First, Boundary::Before);
    order.insert_before(Update, Boundary::BeforeEnd);
    order.insert_after(Update, Boundary::After);
    order.insert_after(Last, Boundary::AfterEnd);
}
fn before(world: &mut World) {
    // The source is the PREVIOUS measured Update: this work lies in its gap.
    let token = world.resource::<FrameSource>().0.and_then(|tag| {
        world
            .resource::<Timeline>()
            .begin(tag, "main_before_update_next")
    });
    world.resource_mut::<Tokens>().before = token;
}
fn before_end(world: &mut World) {
    let token = world.resource_mut::<Tokens>().before.take();
    world.resource::<Timeline>().end(token);
}
fn after(world: &mut World) {
    let token = world
        .resource::<FrameSource>()
        .0
        .and_then(|tag| world.resource::<Timeline>().begin(tag, "main_after_update"));
    world.resource_mut::<Tokens>().after = token;
}
fn after_end(world: &mut World) {
    let token = world.resource_mut::<Tokens>().after.take();
    world.resource::<Timeline>().end(token);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timeline::FrameTag;
    use std::time::Instant;
    #[test]
    fn before_work_belongs_to_previous_update_and_warmup_has_no_tag() {
        let epoch = Instant::now();
        let recorder = Timeline::default();
        let mut app = App::new();
        app.insert_resource(recorder.clone())
            .init_resource::<FrameSource>();
        install(&mut app);
        app.add_systems(Update, move |mut source: ResMut<FrameSource>| {
            source.0 = Some(FrameTag {
                row: source.0.map_or(0, |tag| tag.row + 1),
                generation: 0,
                tick: 0,
                epoch,
            });
        });
        app.update();
        app.update();
        let capture = recorder.close(epoch);
        let actual: Vec<_> = capture
            .spans
            .iter()
            .map(|s| (s.tag.row, s.stage, s.end_ns.is_some()))
            .collect();
        assert_eq!(
            actual,
            [
                (0, "main_after_update", true),
                (0, "main_before_update_next", true),
                (1, "main_after_update", true)
            ]
        );
        app.update(); // A closed capture cannot resume or grow.
        assert!(recorder.close(epoch).spans.is_empty());
    }
}
