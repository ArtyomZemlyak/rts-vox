//! Derived terrain and rigid load-bearing support for stationary buildings.
use super::*;

pub(super) fn tuning() -> &'static modding::foundation::Foundation {
    static T: std::sync::OnceLock<modding::foundation::Foundation> = std::sync::OnceLock::new();
    T.get_or_init(|| {
        modding::foundation::Foundation::from_ron(include_str!(
            "../../../data/range-foundation.ron"
        ))
        .expect("foundation tuning")
    })
}

pub(super) fn blocks(layout: &RangeLayout) -> Vec<modding::range::RangeBlock> {
    let t = tuning();
    layout
        .units
        .iter()
        .map(|u| (u.kind, u.at))
        .chain((!layout.battle).then_some((layout.gun_kind, layout.gun)))
        .filter(|(kind, _)| kind.emplacement())
        .map(|(kind, at)| {
            let unit = RangeUnit::of_kind(kind, at);
            let shape = unit.motion.probe_shape(&unit.body);
            let half =
                std::array::from_fn::<_, 2, _>(|a| shape.half[a].max(shape.support[a]) + t.margin);
            modding::range::RangeBlock {
                min: [at[0] - half[0], (at[1] - t.depth).max(0), at[2] - half[1]],
                max: [at[0] + half[0] + 1, at[1], at[2] + half[1] + 1],
                material: t.material,
            }
        })
        .collect()
}
