//! Exact 100 mm world-cell inspection, independent of object selection/grid.
use super::*;

pub fn select(lab: &mut Laboratory, scene: &Scene, at: [i32; 3]) {
    if (0..3).any(|a| at[a] < 0 || at[a] >= scene.sim.size()[a] as i32) {
        return;
    }
    lab.tools.selected = Some(at);
    lab.tools.falling = None;
    lab.tools.fragment = None;
    lab.tools.probe_stamp = None;
    lab.ui.tab = 14;
    refresh(lab, scene);
}
pub fn refresh(lab: &mut Laboratory, scene: &Scene) {
    let Some(mut at) = lab.tools.selected else {
        return;
    };
    let stamp = (scene.generation, scene.sim.tick);
    if lab.tools.probe_stamp == Some(stamp) {
        return;
    }
    let mut detached = None;
    if let Some(hit) = lab.tools.fragment {
        let fragment = (hit.generation == scene.generation)
            .then(|| {
                scene
                    .sim
                    .matter
                    .debris
                    .fragments
                    .iter()
                    .find(|f| f.id == hit.id)
            })
            .flatten();
        if let Some((f, point)) = fragment.and_then(|f| hit.point(f).map(|p| (f, p))) {
            at = point.floor().to_array().map(|v| v as i32);
            lab.tools.selected = Some(at);
            let speed = Vec3::from(
                [f.velocity.x, f.velocity.y, f.velocity.z]
                    .map(|v| v.raw() as f32 / fx::ONE_RAW as f32),
            )
            .length()
                * voxel::unit::WORLD_MM as f32
                * sim_core::TICKS_PER_SECOND as f32
                / 1000.0;
            detached=Some((f.material,format!("Осколок #{}\nСостояние: {}\nСкорость: {:.2} м/с\nМасса: {} г · клеток: {}\nРазмер клетки осколка: {} мм",f.id,if f.resting {"лежит"}else{"движется"},speed,f.mass_g,f.count,f.mm)));
        } else {
            lab.tools.probe = format!(
                "Осколок #{} удалён или его геометрия изменилась. Выберите осколок заново.",
                hit.id
            );
            lab.tools.probe_stamp = Some(stamp);
            return;
        }
    }
    if let Some((id, local)) = lab.tools.falling {
        if let Some(b) = scene.sim.bodies.list.iter().find(|b| b.id == id) {
            if let Some(&v) = b.voxels.iter().find(|v| [v.x, v.y, v.z] == local) {
                let (x, y, z) = b.world_of(v);
                at = [x as i32, y as i32, z as i32];
                lab.tools.selected = Some(at);
                detached = Some((v.mat, format!("Отделившийся кусок #{id}\nСкорость Y: {:.2} м/с\nПройдено вниз: {} см\nТепло отдельного куска не хранится", b.vy.raw() as f64 as f32 / fx::ONE_RAW as f32 * voxel::unit::WORLD_MM as f32 * sim_core::TICKS_PER_SECOND as f32 / 1000.0, b.fallen * u32::from(voxel::unit::WORLD_MM) / 10)));
            }
        } else {
            lab.tools.falling = None;
        }
    }
    lab.tools.probe = describe(lab, &scene.sim, at, detached);
    lab.tools.probe_stamp = Some(stamp);
}
fn yes(value: bool) -> &'static str {
    if value {
        "да"
    } else {
        "нет"
    }
}
fn threshold(value: u32) -> String {
    if value == 0 {
        "не задан / не применяется".into()
    } else {
        format!("{value} ед. тепла")
    }
}
pub(super) fn describe(
    lab: &Laboratory,
    sim: &BlastRange,
    at: [i32; 3],
    detached: Option<(u16, String)>,
) -> String {
    let Some(info) = at
        .iter()
        .all(|v| *v >= 0)
        .then(|| sim.inspect_cell(at.map(|v| v as u32)))
        .flatten()
    else {
        return "Кубик за границами мира".into();
    };
    let mut material = if info.water > 0 { 11 } else { info.material };
    let mut owner = "Клетка рельефа / размещённого блока".to_owned();
    let mut heat = info.heat.heat;
    let mut work = info.deformation_work;
    let separate = detached.is_some();
    if let Some((mat, description)) = detached {
        material = mat;
        owner = description;
    } else if let Some((id, u, part, cell)) = sim
        .units
        .iter()
        .chain((!sim.point_gun()).then_some(&sim.gun))
        .enumerate()
        .find_map(|(id, u)| u.hit(at).map(|(part, cell)| (id, u, part, cell)))
    {
        let body = if part == 0 {
            &u.body
        } else {
            &u.body.segments[part - 1].body
        };
        let role = body.part_at(cell[0], cell[1], cell[2]);
        material = 3;
        work = if part == 0 {
            u.dents.get(&cell.map(|v| v as u8)).copied().unwrap_or(0)
        } else {
            u.seg_dent(part - 1, cell)
        };
        heat = sim.thermals.heat(sim_core::blast::thermal::Cell {
            unit: id as u16,
            part: part as u8,
            at: cell.map(|v| v as u8),
        });
        owner = format!("Юнит #{id} · сегмент {part}\nРоль: {} · воксель тела {} мм\nЛокально: {} / {} / {}\nСопротивление урону: {}‰\nСопротивление разлому: {}‰\nЮнит работает: {}",role.name(),body.mm,cell[0],cell[1],cell[2],body.ablation_milli[role as usize],body.fracture_milli[role as usize],yes(u.status.alive));
    }
    let name = if material == 0 {
        "Воздух / пустота"
    } else {
        lab.definition
            .materials
            .iter()
            .find(|m| m.id == material)
            .map_or(
                match material { 7 => "Грунт с травой", 13 => "Расплавленная сталь", 14 => "Расплавленный бетон", 16 => "Расплавленное стекло", _ => "Материал" },
                |m| m.name.as_str(),
            )
    };
    let wet = if separate {
        "не хранится для отдельного куска".into()
    } else if info.moisture_capacity == 0 {
        "не впитывает".into()
    } else {
        format!(
            "{} / {} ({}%)",
            info.moisture,
            info.moisture_capacity,
            info.moisture * 100 / info.moisture_capacity
        )
    };
    let phase = match material {
        0 => "газ / пустота",
        11 | 13 | 14 | 16 => "жидкость",
        _ => "твёрдый материал",
    };
    let edge = voxel::unit::WORLD_MM;
    let mut text = format!("КУБ МИРА · {} × {} × {} см\n{name} · ID {material}\nX {} · Y {} · Z {}\nНачало, мм: {} / {} / {}\nОбъём: {} см³\n{owner}\n\nСОСТОЯНИЕ · тик {}\nФаза: {phase}\nНагрев: {heat} ед. тепла\nГорение: {} тиков\nРабота деформации: {work}\nВлажность: {wet}\nВода: {} / {}\nКопка в очереди: {}\nЗарезервировано разрушением: {}\nГраница мира: {}",edge/10,edge/10,edge/10,at[0],at[1],at[2],i64::from(at[0])*i64::from(edge),i64::from(at[1])*i64::from(edge),i64::from(at[2])*i64::from(edge),(edge/10).pow(3),sim.tick,info.heat.burn,info.water,info.water_capacity,yes(info.pending_dig),yes(info.claimed),yes(info.frame));
    if separate {
        text=format!("КУБ МИРА · {} × {} × {} см\n{name} · ID {material}\nX {} · Y {} · Z {}\n{owner}\n\nСОСТОЯНИЕ · тик {}\nФаза: {phase}\nНагрев и влажность для этого куска отдельно не отслеживаются. Свойства грунта под ним не используются.",edge/10,edge/10,edge/10,at[0],at[1],at[2],sim.tick);
    }
    if let Some(m) = sim.matter.economy.mats.get(material) {
        text += &format!("\n\nСВОЙСТВА МАТЕРИАЛА\nПлотность: {}\nПрочность: {}\nПролёт опоры: {} клеток\nСыпучий: {}\nВредит при касании: {}\nТепло горения: {}\nПорог воспламенения: {}\nПорог плавления: {}",m.density,m.strength,m.bend_span,yes(m.loose),yes(m.harmful),m.fuel,threshold(m.kindling),threshold(m.melt));
    }
    if let Some((speed, fraction)) = info.motion.filter(|_| !separate) {
        text += &format!(
            "\nСвободное падение: активно\nСкорость вниз: {:.3} м/с\nДробный путь: {:.2} см",
            speed.raw() as f64 / fx::ONE_RAW as f64
                * f64::from(edge)
                * sim_core::TICKS_PER_SECOND as f64
                / 1000.0,
            fraction.raw() as f64 / fx::ONE_RAW as f64 * f64::from(edge) / 10.0
        );
    }
    if let (Some(repose), Some(pressure)) = (
        info.repose_drop.filter(|_| !separate),
        info.yield_pressure.filter(|_| !separate),
    ) {
        text +=
            &format!("\nПерепад осыпания с влагой: {repose}\nПорог сдвига давлением: {pressure}");
    }
    text += "\n\nЧисла прочности, плотности и давления — единицы движка; нагрев не равен °C.";
    text
}
pub fn shift(lab: &mut Laboratory, scene: &Scene, axis: usize, delta: i32) {
    let Some(mut at) = lab.tools.selected else {
        lab.notice = "Сначала выберите кубик пипеткой".into();
        return;
    };
    if axis < 3 {
        at[axis] = at[axis].saturating_add(delta);
        select(lab, scene, at);
    }
}

pub fn focus(lab: &mut Laboratory) {
    let Some(at) = lab.tools.selected else {
        lab.notice = "Сначала выберите кубик".into();
        return;
    };
    let centre = Vec3::from(at.map(|v| v as f32)) + Vec3::splat(0.5);
    lab.tools.camera = Some(
        Transform::from_translation(
            centre
                - lab
                    .tools
                    .probe_direction
                    .unwrap_or(Vec3::new(-6.0, -5.0, -6.0).normalize())
                    * 10.0,
        )
        .looking_at(centre, Vec3::Y),
    );
    lab.tools.flying = true;
}
