//! Docked editor chrome. Fixed reusable card/scene slots avoid rebuilding UI on hover.
use super::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use modding::laboratory::catalog::{Category, Element};

pub struct State {
    pub tab: u8,
    pub editing: Option<(u8, String, bool)>,
    pub edit_error: Option<String>,
    pub event: Option<usize>,
    pub parameter_details: bool,
    pub category: u8,
    pub page: usize,
    pub grid: bool,
    pub help: bool,
    scale: Option<f32>,
    pub capture_at: Option<[i32; 3]>,
    ready_frames: u8,
    pub viewport_ready: u8,
    pub scenario_input:usize,
    pub device:usize,
    per_page: usize,
}
impl Default for State {
    fn default() -> Self {
        Self {
            tab: 0,
            editing: None,
            edit_error: None,
            event: None,
            parameter_details: false,
            category: 0,
            page: 0,
            grid: false,
            help: false,
            scale: None,
            capture_at: None,
            ready_frames: 0,
            viewport_ready: 0,
            scenario_input:0,
            device:0,
            per_page: 8,
        }
    }
}
#[derive(Component)]
pub struct Inspector(u8);
#[derive(Component)]
pub struct Help;
#[derive(Component)]
pub struct Card(usize);
#[derive(Component)]
pub struct CardImage(usize);
#[derive(Component)]
pub struct CardName(usize);
#[derive(Component)]
pub struct Object(usize);
#[derive(Component)]
pub struct ObjectName(usize);
#[derive(Component)]
pub struct ControlText(LabAction);
#[derive(Component)]
pub struct Icon(&'static str);
#[derive(Resource, Default)]
pub struct Pictures {
    icons: Vec<(&'static str, Handle<Image>)>,
    assets: Vec<(Element, Handle<Image>)>,
    scenarios: Vec<(usize, Handle<Image>)>,
    event_icons: Vec<(&'static str, Handle<Image>)>,
}
const SLOTS: usize = 16;
const CATEGORIES: [&str; 9] = [
    "Все",
    "Блоки",
    "Постройки",
    "Юниты",
    "Природа",
    "Щиты",
    "Мои элементы",
    "Воздействия",
    "Сценарии",
];
fn category(e: &Element) -> u8 {
    match e.category {
        Category::Block => 1,
        Category::Structure => 2,
        Category::Unit => 3,
        Category::Vegetation => 4,
        Category::Shield => 5,
        Category::Custom => 6,
    }
}
fn filtered(lab: &Laboratory) -> Vec<usize> {
    if lab.ui.category==7 {return (0..lab.events.len()).collect();}
    if lab.ui.category==8 {return (0..lab.experiments.len()).collect();}
    lab.constructor
        .catalog
        .iter()
        .enumerate()
        .filter(|(_, e)| lab.ui.category == 0 || category(e) == lab.ui.category)
        .map(|(i, _)| i)
        .collect()
}
fn font(base: &TextFont, size: f32) -> TextFont {
    TextFont {
        font_size: FontSize::Px(size),
        ..base.clone()
    }
}
fn caption(p: &mut ChildSpawnerCommands, f: &TextFont, text: &str) {
    p.spawn((Text::new(text), font(f, 12.0), TextColor(theme::MUTED)));
}
fn heading(p: &mut ChildSpawnerCommands, f: &TextFont, text: &str) {
    p.spawn((
        Text::new(text),
        font(f, 16.0),
        TextColor(theme::TEXT),
        Node {
            margin: UiRect::top(px(5)),
            ..default()
        },
    ));
}
fn icon(p: &mut ChildSpawnerCommands, name: &'static str) {
    p.spawn((
        Icon(name),
        ImageNode::default(),
        Node {
            width: px(19),
            height: px(19),
            flex_shrink: 0.0,
            ..default()
        },
    ));
}
pub(super) fn control(
    p: &mut ChildSpawnerCommands,
    f: &TextFont,
    name: &str,
    image: &'static str,
    a: LabAction,
) {
    p.spawn((
        Button,
        a,
        Node {
            min_height: px(37),
            padding: UiRect::axes(px(9), px(7)),
            column_gap: px(7),
            align_items: AlignItems::Center,
            border: UiRect::all(px(1)),
            border_radius: BorderRadius::all(px(5)),
            ..default()
        },
        BackgroundColor(theme::BUTTON),
        BorderColor::all(theme::FRAME),
    ))
    .with_children(|p| {
        icon(p, image);
        let mut text = p.spawn((
            ControlText(a),
            Text::new(name),
            f.clone(),
            TextColor(theme::TEXT),
        ));
        if a == LabAction::Pause {
            text.insert(parameters::Info::Pause);
        }
    });
}
fn controls(p: &mut ChildSpawnerCommands, f: &TextFont, items: &[(&str, &'static str, LabAction)]) {
    p.spawn(Node {
        column_gap: px(6),
        row_gap: px(6),
        flex_wrap: FlexWrap::Wrap,
        ..default()
    })
    .with_children(|p| {
        for &(name, image, a) in items {
            control(p, f, name, image, a);
        }
    });
}
fn panel_node() -> Node {
    Node {
        position_type: PositionType::Absolute,
        padding: UiRect::all(px(12)),
        row_gap: px(10),
        flex_direction: FlexDirection::Column,
        border: UiRect::all(px(1)),
        border_radius: BorderRadius::all(px(7)),
        display: Display::None,
        ..default()
    }
}

fn section(p: &mut ChildSpawnerCommands, id: u8, children: impl FnOnce(&mut ChildSpawnerCommands)) {
    p.spawn((
        Inspector(id),
        Node {
            width: percent(100),
            flex_direction: FlexDirection::Column,
            row_gap: px(10),
            ..default()
        },
    ))
    .with_children(children);
}

fn editing(lab: &Laboratory) -> Option<&Element> {
    let c = &lab.constructor;
    (!matches!(c.mode, constructor::Mode::Place | constructor::Mode::Part | constructor::Mode::Drill | constructor::Mode::Crawler | constructor::Mode::Collector))
        .then(|| c.selected.and_then(|i| c.placed.get(i)).map(|p| &p.element))
        .flatten()
}
fn single_block(lab: &Laboratory) -> bool {
    editing(lab).is_some_and(|e| e.blocks.len() == 1 && e.units.is_empty() && e.shields.is_empty())
}
fn section_visible(lab: &Laboratory, id: u8) -> bool {
    match id {
        0..=3 | 9 | 10 | 14 | 16 | 17 | 21 | 24 | 25 | 30 => lab.ui.tab == id,
        4 => lab.constructor.creating(),
        5 => editing(lab).is_some(),
        6 => matches!(
            lab.constructor.mode,
            constructor::Mode::Place | constructor::Mode::Part
        ),
        7 => lab.constructor.mode == constructor::Mode::Part || single_block(lab),
        8 => enabled(lab, LabAction::Split),
        18 => lab.constructor.can_voxel_edit(),
        19 => !lab.constructor.voxel_editing() && !lab.tools.live,
        20 => lab.constructor.voxel_editing(),
        22 => lab.tools.live,
        26 => scenario::has_actor(lab),
        27 => scenario::has_target(lab),
        28 => scenario::has_pilot(lab),
        29 => lab.constructor.workspace.devices.get(lab.ui.device).is_some_and(|d|d.collector.is_some()),
        11 => lab.ui.parameter_details,
        12 => lab.ui.editing.is_some(),
        13 => lab.constructor.can_return(),
        _ => false,
    }
}

pub fn build(commands: &mut Commands, camera: Entity, base: &TextFont) {
    commands.init_resource::<Pictures>();
    let f = font(base, 14.0);
    let mut header = panel_node();
    header.left = px(12);
    header.right = px(12);
    header.top = px(10);
    header.height = px(58);
    header.padding = UiRect::axes(px(12), px(8));
    header.flex_direction = FlexDirection::Row;
    header.justify_content = JustifyContent::SpaceBetween;
    header.align_items = AlignItems::Center;
    commands.spawn(root(header, camera)).with_children(|p| {
        p.spawn(Node {
            column_gap: px(10),
            align_items: AlignItems::Center,
            ..default()
        })
        .with_children(|p| {
            icon(p, "flask-conical");
            heading(p, &f, "ЛАБОРАТОРИЯ");
            label(p, &f, "", Some(14));
        });
        controls(
            p,
            &f,
            &[
                ("Опыты", "flask-conical", LabAction::Experiments),
                ("Стенд · F6", "camera", LabAction::StandView),
                ("Сохранить", "save", LabAction::SaveScene),
                ("Открыть", "folder-open", LabAction::LoadScene),
                ("Помощь", "circle-help", LabAction::Help),
                ("Полигон", "x", LabAction::Back),
            ],
        );
    });
    let mut toolbar = panel_node();
    toolbar.left = px(12);
    toolbar.right = px(12);
    toolbar.top = px(76);
    toolbar.min_height = px(48);
    toolbar.padding = UiRect::axes(px(9), px(5));
    toolbar.flex_direction = FlexDirection::Row;
    toolbar.justify_content = JustifyContent::SpaceBetween;
    commands.spawn(root(toolbar, camera)).with_children(|p| {
        controls(
            p,
            &f,
            &[
                ("Выбор · V", "mouse-pointer-2", LabAction::Tool(0)),
                ("Пипетка · I", "pipette", LabAction::Tool(4)),
                ("Перенос · G", "move", LabAction::Tool(1)),
                ("Деталь · B", "box", LabAction::AddPart),
                ("Цель · T", "crosshair", LabAction::Aim),
                ("−90° · Q", "rotate-ccw", LabAction::Turn(-1)),
                ("+90° · E", "rotate-cw", LabAction::Turn(1)),
                ("Сетка", "grid-3x3", LabAction::Grid),
                ("Разрез · F7", "layers", LabAction::Tab(30)),
                ("Общий вид · Home", "camera", LabAction::Frame),
                ("Живые · F2", "hammer", LabAction::LiveTools),
            ],
        );
        controls(
            p,
            &f,
            &[
                ("Пуск · F", "play", LabAction::Fire),
                ("Пауза", "pause", LabAction::Pause),
                ("Тик", "step-forward", LabAction::Step),
                ("Сброс", "undo-2", LabAction::Reset),
            ],
        );
    });
    let mut left = panel_node();
    left.left = px(12);
    left.top = px(134);
    left.bottom = px(318);
    left.width = px(238);
    commands.spawn(root(left, camera)).with_children(|p| {
        heading(p, &f, "Состав сцены");
        controls(p, &f, &[("Новый элемент", "plus", LabAction::Create)]);
        scrolling::area(p, 0, |p| {
            caption(p, &f, "Объекты выбираются здесь или кликом в сцене.");
            for i in 0..32 {
                p.spawn((
                    Button,
                    Object(i),
                    LabAction::PickObject(i),
                    Node {
                        min_height: px(35),
                        padding: UiRect::all(px(7)),
                        column_gap: px(8),
                        align_items: AlignItems::Center,
                        display: Display::None,
                        border: UiRect::all(px(1)),
                        border_radius: BorderRadius::all(px(4)),
                        ..default()
                    },
                    BackgroundColor(theme::BUTTON),
                    BorderColor::all(theme::FRAME),
                ))
                .with_children(|p| {
                    icon(p, "box");
                    p.spawn((
                        ObjectName(i),
                        Text::new(""),
                        f.clone(),
                        TextColor(theme::TEXT),
                    ));
                });
            }
        });
        controls(
            p,
            &f,
            &[
                ("Копия", "copy", LabAction::Duplicate),
                ("Удалить", "trash-2", LabAction::Delete),
            ],
        );
        label(p, &f, "", Some(18));
        p.spawn((
            Inspector(4),
            Node {
                column_gap: px(6),
                ..default()
            },
        ))
        .with_children(|p| {
            control(p, &f, "Готово", "save", LabAction::SaveElement);
            control(p, &f, "Отмена", "x", LabAction::CancelCreate);
        });
        controls(p, &f, &[("Очистить сцену", "trash-2", LabAction::Clear)]);
        section(p,13,|p| {control(p,&f,"Вернуть мою сборку","undo-2",LabAction::ReturnConstruction);});
    });
    let mut right = panel_node();
    right.right = px(12);
    right.top = px(134);
    right.bottom = px(318);
    right.width = px(330);
    commands.spawn(root(right,camera)).with_children(|p| {
        heading(p,&f,"Инспектор");
        section(p,19,|p|{controls(p,&font(&f,12.0),&[("Объект","box",LabAction::Tab(0)),("Материал","layers",LabAction::Tab(1)),
            ("Оружие","crosshair",LabAction::Tab(2)),("Результат","file-text",LabAction::Tab(3)),("Сцена","settings-2",LabAction::Tab(9)),("Сценарий","file-text",LabAction::Tab(24)),("Устройства","hammer",LabAction::Tab(25)),("Источники","crosshair",LabAction::Tab(10)),("Инструменты","hammer",LabAction::Tab(16)),("Пипетка","pipette",LabAction::Tab(14)),("Кубики","grid-3x3",LabAction::Tab(17))]);});
        section(p,20,|p|{controls(p,&font(&f,12.0),&[("Кубики","grid-3x3",LabAction::Tab(17)),("Материал","layers",LabAction::Tab(1))]);});
        section(p,22,|p|{controls(p,&font(&f,12.0),&[("Инструмент","hammer",LabAction::Tab(21)),("Материал","layers",LabAction::Tab(1))]);});
        scrolling::area(p, 1, |p| {
            for tab in [0,1,2,3,9,10,16,14,17,21,24,25,30] {
                p.spawn((Inspector(tab),Node{width:percent(100),flex_direction:FlexDirection::Column,row_gap:px(if tab==10 {6}else{10}),..default()})).with_children(|p|match tab {
                    30=>{
                        heading(p,&f,"Разрез по плоскости");
                        label(p,&f,"",Some(27));
                        controls(p,&f,&[("Вкл / выкл · F7","layers",LabAction::SectionToggle),("К выбранному","crosshair",LabAction::SectionFocus)]);
                        caption(p,&f,"Плоскость направлена по взгляду камеры и проходит через выбранный куб или объект. Пипеткой выберите куб, затем нажмите «К выбранному».");
                        controls(p,&f,&[("Ближе на 1 м","chevron-left",LabAction::SectionShift(-10)),("Ближе на 10 см","chevron-left",LabAction::SectionShift(-1))]);
                        controls(p,&f,&[("Дальше на 10 см","chevron-right",LabAction::SectionShift(1)),("Дальше на 1 м","chevron-right",LabAction::SectionShift(10))]);
                        caption(p,&f,"Разрез только скрывает видимую часть сцены: физика и кубики не меняются. В режиме вида от первого лица разрез временно отключён.");
                        caption(p,&f,"У юнитов показывается срез их видимой оболочки; отдельная заливка внутренних кубиков модели пока не строится.");
                    },
                    0=>{
                        label(p,&f,"",Some(9));
                        section(p,5,|p| {
                        caption(p,&f,"Позиция · привязка к сетке");
                        for (axis,name) in [(0,"X"),(1,"Y"),(2,"Z")] {
                            controls(p,&f,&[(&format!("{name} −"),"chevron-left",LabAction::Move(axis,-1)),(&format!("{name} +"),"chevron-right",LabAction::Move(axis,1))]);
                        }
                        controls(p,&f,&[("Перенести мышью","move",LabAction::Tool(1)),("Поворот 90°","rotate-cw",LabAction::Turn(1))]);
                        });
                        section(p,6,|p| {
                        caption(p,&f,"Высота при размещении");
                        controls(p,&f,&[("Ниже · PgDn","chevron-left",LabAction::Elevation(-1)),("Выше · PgUp","chevron-right",LabAction::Elevation(1))]);
                        });
                        section(p,7,|p| {
                        heading(p,&f,"Деталь / конструктор");label(p,&f,"",Some(10));
                        for (axis,name) in [(0,"Ширина"),(1,"Высота"),(2,"Глубина")] {
                            controls(p,&f,&[(&format!("{name} −"),"chevron-left",LabAction::Dimension(axis,-1)),(&format!("{name} +"),"chevron-right",LabAction::Dimension(axis,1))]);
                        }
                        controls(p,&f,&[("Размер → объект","settings-2",LabAction::ApplySize)]);
                        });
                        section(p,18,|p| {controls(p,&f,&[("Кубики 10 см","grid-3x3",LabAction::BeginVoxelEdit),("В библиотеку","save",LabAction::SavePart)]);});
                        section(p,8,|p| {
                        controls(p,&f,&[("Разобрать сборку","layers",LabAction::Split)]);
                        });
                        caption(p,&f,"Для нового блока: выберите материал во вкладке «Материал», затем «Деталь · B». Готовые сборки можно разобрать и редактировать по частям.");
                    },
                    1=>{
                        heading(p,&f,"Материал объекта");label(p,&f,"",Some(23));
                        controls(p,&f,&[("Выбор · V","mouse-pointer-2",LabAction::Tool(0)),("Применить","layers",LabAction::ApplyMaterial)]);
                        heading(p,&f,"Выберите новый материал");label(p,&f,"",Some(0));
                        p.spawn(Node {display:Display::Grid,grid_template_columns:vec![RepeatedGridTrack::flex(2,1.0)],column_gap:px(6),row_gap:px(6),..default()}).with_children(|p| {
                            for material in &Definition::stock().materials {control(p,&font(&f,12.0),if material.id==12 {"Стекло"}else{&material.name},"layers",LabAction::PickMaterial(material.id));}
                        });
                        caption(p,&f,"Палитра не меняет сцену до применения. Новая деталь и кисть используют выбранный материал сразу.");
                        heading(p,&f,"Свойства типа материала · вся сцена");
                        for (id,name) in [(30,"Прочность материала"),(31,"Плотность, ×10 кг/м³"),(32,"Порог оплавления")] { parameters::row(p,&f,id,name); }
                        caption(p,&f,"Эти числа применяются ко всем объектам из выбранного материала. Применение замены материала восстанавливает исходную сцену опыта.");
                        heading(p,&f,"Отдельный испытательный стенд");control(p,&f,"Стенд вкл/выкл","layers",LabAction::Stand);label(p,&f,"",Some(1));
                        controls(p,&f,&[("Форма −","chevron-left",LabAction::Form(-1)),("Форма +","chevron-right",LabAction::Form(1))]);
                        label(p,&f,"",Some(2));controls(p,&f,&[("Толщина","settings-2",LabAction::Thickness),("Прочность стенда ×","shield",LabAction::Strength)]);
                    },
                    2=>{
                        parameters::info(p,&f,parameters::Info::Source);
                        controls(p,&font(&f,12.0),&[("Выстрел","crosshair",LabAction::LocalCharge(false)),("Взрыв в точке","bomb",LabAction::LocalCharge(true))]);
                        parameters::info(p,&font(&f,12.0),parameters::Info::Trigger);
                        controls(p,&f,&[("Источники","crosshair",LabAction::Tab(10)),("Инструменты","hammer",LabAction::Tab(16)),("Пипетка","pipette",LabAction::Tab(14)),("Выбрать цель","crosshair",LabAction::Aim)]);
                        parameters::row(p,&f,0,"Мощность выбранного источника, ×");
                        label(p,&f,"",Some(11));heading(p,&f,"Воздействие");
                        p.spawn(Node { display: Display::Grid, grid_template_columns: vec![RepeatedGridTrack::flex(2,1.0)], column_gap:px(6), row_gap:px(6), ..default() }).with_children(|p| {
                        for (i,name) in ["Фугас","Бронебой","Энергопушка","Лазер","Огнемёт","Удар по опоре","Анипушка","Пулемёт","Ядерный заряд","Осадный снаряд"].iter().enumerate() {
                            control(p,&font(&f,13.0),name,match i{0|9=>"bomb",2|6=>"zap",3=>"sparkles",4=>"flame",5=>"hammer",8=>"radiation",_=>"crosshair"},LabAction::Effect(i as u8));
                        }
                        });
                        label(p,&f,"",Some(3));
                        caption(p,&f,"Нажмите число, введите значение и Enter. Esc отменяет ввод. Настройки относятся только к выбранному источнику.");
                        for (id,name) in [(1,"Энергия"),(2,"Скорость снаряда"),(3,"Гравитация опыта"),(4,"Зарядка, тиков"),(5,"Нагрев клетки"),(6,"Энергия разрушения, Дж"),(7,"Импульс корпуса, ‰"),(8,"Работа волны за тик"),(9,"Правки за тик")] {parameters::row(p,&f,id,name);}
                        control(p,&f,"Расширенные параметры","settings-2",LabAction::ParameterDetails);
                        section(p,11,|p| {
                            for (id,name) in [(40,"Радиус нагрева"),(41,"Радиус источника газа"),(42,"Температура газа, K"),(43,"Топливо газа, ‰"),(44,"Давление газа, ‰"),(45,"Охлаждение, ‰"),(46,"Реакция, ‰"),(47,"Плавучесть дыма, ‰"),(48,"Сопротивление грунта"),(49,"Сопротивление бетона"),(50,"Сопротивление стали"),(51,"Скрепление обломков"),(52,"Полуразмер сетки волны"),(53,"Размер ячейки волны"),(54,"Подготовка ячеек за тик")] {parameters::row(p,&f,id,name);}
                        });
                        controls(p,&f,&[("Сохранить оружие","save",LabAction::SaveWeapon),("Моё −","chevron-left",LabAction::Variant(-1)),("Моё +","chevron-right",LabAction::Variant(1))]);
                    },
                    10=>{
                        heading(p,&f,"Источники оружия");
                        caption(p,&f,"Выбор: список или куб в сцене · F — пуск");
                        parameters::source_list(p,&f);
                        controls(p,&f,&[("Добавить","plus",LabAction::AddSource),("Убрать","trash-2",LabAction::RemoveSource)]);
                        controls(p,&font(&f,12.0),&[("Позиция","move",LabAction::SourcePosition),("К юниту","move",LabAction::BindSource(true)),("Точка","crosshair",LabAction::BindSource(false))]);
                        parameters::info(p,&font(&f,12.0),parameters::Info::Binding);
                        controls(p,&font(&f,12.0),&[("Оружие","settings-2",LabAction::Tab(2)),("Цель","crosshair",LabAction::Aim)]);
                        parameters::row(p,&f,0,"Мощность, ×");
                        parameters::coordinates(p,&f,10,"Свободная точка");
                        parameters::coordinates(p,&f,13,"Цель");
                        caption(p,&f,"Правка XYZ точки снимает привязку к юниту.");
                    },
                    16=>{
                        heading(p,&f,"Инструменты физической лаборатории");
                        controls(p,&f,&[("Живые инструменты · F2","play",LabAction::LiveTools)]);
                        caption(p,&f,"Запустить симуляцию и воздействовать мышью: копать, лить воду, направлять огонь и лазер.");
                        controls(p,&f,&[("Материал","plus",LabAction::Tool(2)),("Удаление","trash-2",LabAction::Tool(3)),("Кубик","layers",LabAction::Tool(4))]);
                        controls(p,&f,&[("Бур","hammer",LabAction::Tool(5)),("Краулер-бур","move",LabAction::Tool(6)),("Сборщик","move",LabAction::Tool(7))]);
                        controls(p,&f,&[("Свободный полёт","camera",LabAction::Flying)]);
                        caption(p,&f,"Полёт: WASD · Z вниз · X вверх · удерживайте ПКМ для обзора. Кисть находится перед камерой. ЛКМ — материал / удаление; выбранный материал — во вкладке «Материал».");
                        label(p,&f,"",Some(22));
                        controls(p,&f,&[("Кисть −","chevron-left",LabAction::BrushSize(-1)),("Кисть +","chevron-right",LabAction::BrushSize(1))]);
                        controls(p,&f,&[("Ближе","chevron-left",LabAction::BrushDistance(-1)),("Дальше","chevron-right",LabAction::BrushDistance(1))]);
                        heading(p,&f,"Размещение");
                        controls(p,&f,&[("К поверхности","move",LabAction::SurfaceSnap),("Пересечения","layers",LabAction::AllowOverlap)]);
                        caption(p,&f,"Активная кнопка подсвечена. Привязка ставит грань к грани: можно собирать башни и приставлять детали сбоку. Пересечения разрешают наложение элементов. Физические столкновения продолжают работать.");
                        caption(p,&f,"Бур идёт вниз, краулер режет породу перед собой и движется по опоре. Q/E поворачивает направление до установки. Пауза / Тик управляют проверкой; добытое остаётся в контейнере устройства.");
                    },
                    24=>{
                        heading(p,&f,"Команды сценария");
                        caption(p,&f,"F запускает весь сценарий. Источники — обычные точки оружия: выберите строку, чтобы изменить позицию, мощность и параметры. Порядок команд — сверху вниз.");
                        scenario::list(p,&font(&f,12.0),0);
                        controls(p,&font(&f,12.0),&[("Выше","chevron-left",LabAction::ScenarioMove(-1)),("Ниже","chevron-right",LabAction::ScenarioMove(1)),("Удалить","trash-2",LabAction::ScenarioRemove),("Источники","crosshair",LabAction::Tab(10))]);
                        label(p,&f,"",Some(26));
                        section(p,26,|p| {parameters::row(p,&f,60,"Участник № (по порядку юнитов)");});
                        section(p,27,|p| {parameters::coordinates(p,&f,61,"Точка / направление команды");
                            controls(p,&font(&f,12.0),&[("Выбрать точку мышью","crosshair",LabAction::Aim)]);});
                        section(p,28,|p| {parameters::row(p,&f,64,"Ход −1…1 · управление");
                            parameters::row(p,&f,65,"Поворот −1…1 · управление");
                            parameters::row(p,&f,66,"Огонь 0/1 · управление");});
                        heading(p,&f,"Добавить команду");
                        controls(p,&font(&f,12.0),&[("Выстрел","crosshair",LabAction::ScenarioAdd(0)),("Заряд","bomb",LabAction::ScenarioAdd(1)),("Движение","move",LabAction::ScenarioAdd(4)),("Копка","hammer",LabAction::ScenarioAdd(5)),("Орудие юнита","crosshair",LabAction::ScenarioAdd(6)),("Управление","settings-2",LabAction::ScenarioAdd(7))]);
                        controls(p,&font(&f,12.0),&[("Ручные приказы","mouse-pointer-2",LabAction::ScenarioAdd(2)),("Начало боя","play",LabAction::ScenarioAdd(3))]);
                        parameters::row(p,&f,23,"Наблюдение, тиков");
                        caption(p,&f,"Движение, копка и оружие юнита требуют команду «Ручные приказы», стрельба в бою — также «Начало боя». У каждой команды свободного оружия используется её источник, у участника — его настоящий ствол. Изменения объектов и команд применяются после сброса к исходной сцене.");
                    },
                    25=>{
                        heading(p,&f,"Устройства сценария");
                        scenario::list(p,&font(&f,12.0),1);
                        controls(p,&font(&f,12.0),&[("Удалить устройство","trash-2",LabAction::RemoveDevice)]);
                        parameters::coordinates(p,&f,70,"Исходная позиция · 10 см");
                        parameters::row(p,&f,73,"Поворот: 0, 1, 2, 3");
                        section(p,29,|p| {heading(p,&f,"Сборщик обломков");
                            parameters::row(p,&f,74,"Вместимость бака");
                            parameters::coordinates(p,&f,75,"Площадка разгрузки");
                            parameters::coordinates(p,&f,78,"Центр зоны сбора");});
                        caption(p,&f,"Геометрия, устройства и физические материалы доступны для редактирования во всех готовых опытах. Песок и вода — обычные блоки сцены; выберите их в составе слева. Новые устройства размещаются во вкладке «Инструменты».");
                        controls(p,&font(&f,12.0),&[("Разместить устройство","hammer",LabAction::Tab(16)),("Сценарий","file-text",LabAction::Tab(24))]);
                    },
                    21=>{
                        heading(p,&f,"Живые инструменты");
                        controls(p,&font(&f,12.0),&[("От первого лица · Tab","camera",LabAction::LiveCapture)]);
                        controls(p,&font(&f,12.0),&[("Готово · Esc","x",LabAction::LiveTools),("Пауза · Space","pause",LabAction::Pause)]);
                        controls(p,&font(&f,12.0),&[("Копатель","hammer",LabAction::LiveTool(0)),("Материал","plus",LabAction::LiveTool(1)),("Вода","layers",LabAction::LiveTool(2)),("Огонь","flame",LabAction::LiveTool(3)),("Лазер","zap",LabAction::LiveTool(4))]);
                        controls(p,&font(&f,12.0),&[("Пулемёт · 6","crosshair",LabAction::LiveTool(5)),("Фугас · 7","flame",LabAction::LiveTool(6)),("Бронебой · 8","crosshair",LabAction::LiveTool(7))]);
                        controls(p,&font(&f,12.0),&[("Точка в воздухе","crosshair",LabAction::LivePoint),("Полёт","camera",LabAction::Flying)]);
                        label(p,&font(&f,12.0),"",Some(25));
                        controls(p,&font(&f,12.0),&[("Кисть −","chevron-left",LabAction::BrushSize(-1)),("Кисть +","chevron-right",LabAction::BrushSize(1)),("Ближе","chevron-left",LabAction::BrushDistance(-1)),("Дальше","chevron-right",LabAction::BrushDistance(1))]);
                        controls(p,&font(&f,12.0),&[("Мощность −","chevron-left",LabAction::LivePower(-1)),("Мощность +","chevron-right",LabAction::LivePower(1))]);
                        controls(p,&font(&f,12.0),&[("Полёт −","chevron-left",LabAction::LiveSpeed(-1)),("Полёт +","chevron-right",LabAction::LiveSpeed(1))]);
                        controls(p,&font(&f,12.0),&[("Мышь −","chevron-left",LabAction::LiveLook(-1)),("Мышь +","chevron-right",LabAction::LiveLook(1)),("FOV −","chevron-left",LabAction::LiveFov(-1)),("FOV +","chevron-right",LabAction::LiveFov(1))]);
                        caption(p,&f,"Tab — полёт от первого лица / панели. 1–8 — инструменты; WASD — движение, Space/Ctrl — высота, Shift — быстрее, Alt — точно, ПКМ — точный обзор. Колесо — дальность, Shift+колесо — скорость; Q/E — кисть, R — точка/поверхность, P — пауза. Удерживайте ЛКМ: воздействие следует за прицелом. Z/X также меняют высоту. «Точка в воздухе» создаёт источник перед камерой; выключено — работа по поверхности. Палитра во вкладке «Материал». Сцена продолжает жить, падать, течь и гореть. Размер куба — 10 см.");
                    },
                    17=>{
                        heading(p,&f,"Редактор детали · 10 см");
                        controls(p,&font(&f,12.0),&[("Готово","save",LabAction::VoxelFinish(true)),("Отмена","x",LabAction::VoxelFinish(false))]);
                        controls(p,&font(&f,12.0),&[("Добавить","plus",LabAction::VoxelTool(0)),("Удалить","trash-2",LabAction::VoxelTool(1)),("Красить","layers",LabAction::VoxelTool(2))]);
                        label(p,&f,"",Some(24));
                        controls(p,&font(&f,12.0),&[("Отменить мазок","undo-2",LabAction::VoxelUndo),("Вернуть","rotate-cw",LabAction::VoxelRedo)]);
                        controls(p,&font(&f,12.0),&[("Деталь целиком · F","camera",LabAction::Frame),("Материал","layers",LabAction::Tab(1))]);
                        controls(p,&font(&f,12.0),&[("В библиотеку","save",LabAction::SavePart)]);
                        caption(p,&f,"Осмотр детали: средняя кнопка мыши — вращение вокруг центра; Shift + средняя — сдвиг; колесо — масштаб; Ctrl + средняя — плавный масштаб. F / Home / Num . — деталь целиком. ЛКМ — мазок по кубикам 10 см, отпускание — применить. ПКМ отменяет незавершённый мазок. Добавление — грань к грани; покраска — только затронутые клетки. Физика на паузе. Esc — готово.");
                    },
                    14=>{
                        heading(p,&f,"Пипетка · куб мира 10 см");
                        controls(p,&font(&f,12.0),&[("Приблизить","camera",LabAction::ProbeFocus),("Снять выбор","x",LabAction::ClearProbe)]);
                        label(p,&f,"",Some(21));
                        caption(p,&f,"I → ЛКМ по поверхности. Золотым светится поверхность выбранного кубика 10 см. Состояние обновляется с симуляцией.");
                        caption(p,&f,"Соседняя клетка · шаг 10 см, включая пустоты и глубину материала");
                        for (a,n) in [(0,"X"),(1,"Y"),(2,"Z")] {controls(p,&f,&[(&format!("{n} −"),"chevron-left",LabAction::ProbeMove(a,-1)),(&format!("{n} +"),"chevron-right",LabAction::ProbeMove(a,1))]);}
                    },
                    9=>{
                        heading(p,&f,"Карта и симуляция");
                        caption(p,&f,"Размеры — в мировых единицах, кратны 16. Ребро куба не меняется. Изменение карты восстанавливает исходную расстановку.");
                        for (id,name) in [(20,"Ширина карты X"),(21,"Высота карты Y"),(22,"Длина карты Z"),(23,"Наблюдение, тиков"),(24,"Шаг привязки"),(25,"Стойкость корпусов")] {parameters::row(p,&f,id,name);}
                        caption(p,&f,"Число меняется кнопками −/+ или вводом с Enter. Неверный размер сохраняет прежнюю карту и объекты.");
                    },
                    _=>{
                        heading(p,&f,"Измерения опыта");label(p,&f,"",Some(4));
                        controls(p,&f,&[("Повторить · F5","play",LabAction::Repeat),("Запомнить A","copy",LabAction::Keep),("Сохранить отчёт","file-text",LabAction::Save)]);
                        label(p,&f,"",Some(5));
                    }
                });
            }
        });
        section(p,12,|p| {
            parameters::info(p,&font(&f,12.0),parameters::Info::Edit);
            controls(p,&font(&f,12.0),&[("Применить · Enter","save",LabAction::ApplyNumber),("Отмена · Esc","x",LabAction::CancelNumber)]);
        });
        caption(p,&f,"Прокрутка: колесо или полоса справа");
    });
    let mut status = panel_node();
    status.left = px(262);
    status.right = px(354);
    status.bottom = px(270);
    status.min_height = px(38);
    status.padding = UiRect::all(px(9));
    commands
        .spawn(root(status, camera))
        .with_children(|p| label(p, &f, "", Some(12)));
    let mut browser = panel_node();
    browser.left = px(12);
    browser.right = px(12);
    browser.bottom = px(10);
    browser.height = px(250);
    browser.row_gap = px(8);
    commands.spawn(root(browser, camera)).with_children(|p| {
        p.spawn(Node {
            justify_content: JustifyContent::SpaceBetween,
            align_items: AlignItems::Center,
            ..default()
        })
        .with_children(|p| {
            p.spawn(Node {
                column_gap: px(7),
                align_items: AlignItems::Center,
                ..default()
            })
            .with_children(|p| {
                heading(p, &f, "Библиотека");
                for (i, name) in CATEGORIES.iter().enumerate() {
                    control(
                        p,
                        &f,
                        name,
                        match i {
                            3 => "move",
                            4 => "tree-pine",
                            5 => "shield",
                            7 => "bomb",
                            8 => "flask-conical",
                            _ => "box",
                        },
                        LabAction::Category(i as u8),
                    );
                }
            });
            controls(
                p,
                &f,
                &[
                    ("Назад", "chevron-left", LabAction::Page(-1)),
                    ("Дальше", "chevron-right", LabAction::Page(1)),
                ],
            );
        });
        p.spawn(Node {
            column_gap: px(8),
            height: px(142),
            ..default()
        })
        .with_children(|p| {
            for i in 0..SLOTS {
                p.spawn((
                    Button,
                    Card(i),
                    LabAction::PickCatalog(0),
                    Node {
                        width: px(145),
                        height: px(142),
                        flex_shrink: 0.0,
                        padding: UiRect::all(px(4)),
                        row_gap: px(4),
                        flex_direction: FlexDirection::Column,
                        border: UiRect::all(px(2)),
                        border_radius: BorderRadius::all(px(5)),
                        overflow: Overflow::clip(),
                        ..default()
                    },
                    BackgroundColor(theme::BUTTON),
                    BorderColor::all(theme::FRAME),
                ))
                .with_children(|p| {
                    p.spawn((
                        CardImage(i),
                        ImageNode::default(),
                        Node {
                            width: percent(100),
                            height: px(94),
                            ..default()
                        },
                    ));
                    p.spawn((
                        CardName(i),
                        Text::new(""),
                        font(&f, 13.0),
                        TextColor(theme::TEXT),
                    ));
                });
            }
        });
        label(p, &font(&f, 12.0), "", Some(13));
        p.spawn((
            Label(15),
            Text::new(""),
            f.clone(),
            TextColor(theme::MUTED),
            Node {
                position_type: PositionType::Absolute,
                left: px(22),
                right: px(22),
                top: px(102),
                ..default()
            },
        ));
    });
    experiments::build(commands, camera, &f);
    let mut help = panel_node();
    help.left = percent(28);
    help.right = percent(28);
    help.top = percent(22);
    help.padding = UiRect::all(px(22));
    commands.spawn((root(help,camera),Help)).with_children(|p| {
        heading(p,&f,"Быстрый старт");
        label(p,&f,"1. Выберите карточку в библиотеке снизу.\n2. Наведите на сцену: зелёный — можно разместить, красный — нельзя.\n3. Q / E или R — поворот на 90°. Page Up / Down — высота.\n4. ЛКМ — поставить; ПКМ / Esc — завершить размещение.\n5. V — выбор, G — перенос выбранного объекта мышью. I — пипетка кубика 10 см.\n6. Ctrl+D — копия, Delete — удалить. Ctrl+S — сохранить сцену.\n7. «Источники»: добавьте точку и поставьте её кнопкой «Позиция».\n8. Для ствола модели выберите юнит слева и нажмите «К юниту».\n9. «Оружие»: настройте выбранный источник и цель (T). F запускает выбранный источник; в готовом сценарии — все его команды по порядку.\n10. «Сцена»: размер карты, шаг сетки и длительность наблюдения.\n11. Выберите объект → «Материал» → материал по имени → «Применить к цели». Для части сборки сначала «Разобрать сборку».\n\nБиблиотека «Воздействия»: выберите взрыв → поставьте в сцене → F. PgUp/PgDn задают высоту. «Сценарии» открывают обвалы, падения, огонь и другие готовые проверки; «Вернуть мою сборку» возвращает вашу сцену.\n\nЧисла: клик → ввод → Enter или «Применить». Esc отменяет; Ctrl+A выделяет всё. При ошибке значение остаётся для исправления. Пробел — пауза, F5 — повторить.\n\nКамера: колёсико — масштаб; средняя кнопка + мышь — перемещение; Shift + средняя кнопка — поворот; Home — общий вид.\nEsc закрывает подсказку или отменяет инструмент. Кнопка «Полигон» возвращает прежнюю сессию.",None);
        controls(p,&f,&[("Камера","camera",LabAction::Camera),("Понятно · Esc","x",LabAction::Help)]);
    });
}

pub fn selected(lab: &Laboratory, a: LabAction) -> bool {
    let c = &lab.constructor;
    match a {
        LabAction::ScenarioInput(i)=>lab.ui.scenario_input==i,
        LabAction::Device(i)=>lab.ui.device==i,
        LabAction::PickEvent(i)=>lab.ui.event==Some(i)&&c.mode==constructor::Mode::Event,
        LabAction::Experiment(i)=>lab.experiments.get(i).is_some_and(|e|c.experiment.as_ref().is_some_and(|p|p.name==e.plan.name)),
        LabAction::LocalCharge(local)=>sources::active(lab).is_some_and(|s|s.detonate==local),
        LabAction::EditNumber(id) => lab.ui.editing.as_ref().is_some_and(|(i, _, _)| *i == id),
        LabAction::SourcePosition => c.mode == constructor::Mode::Source,
        LabAction::Source(i) => c.workspace.selected == i,
        LabAction::BindSource(bind) => {
            sources::active(lab).is_some_and(|s| s.binding.is_some() == bind)
        }
        LabAction::VoxelTool(n) => c.voxel_edit.as_ref().is_some_and(|s|s.tool==match n {0=>modding::laboratory::catalog::voxels::Tool::Add,1=>modding::laboratory::catalog::voxels::Tool::Erase,_=>modding::laboratory::catalog::voxels::Tool::Paint}),
        LabAction::Tool(0) => c.mode == constructor::Mode::Select,
        LabAction::Tool(1) => c.mode == constructor::Mode::Move,
        LabAction::Tool(2)=>c.mode==constructor::Mode::Brush,
        LabAction::Tool(3)=>c.mode==constructor::Mode::Erase,
        LabAction::Tool(4)=>c.mode==constructor::Mode::Inspect,
        LabAction::Tool(5)=>c.mode==constructor::Mode::Drill,
        LabAction::Tool(6)=>c.mode==constructor::Mode::Crawler,
        LabAction::Tool(7)=>c.mode==constructor::Mode::Collector,
        LabAction::Flying=>lab.tools.flying,
        LabAction::LiveTools=>lab.tools.live,
        LabAction::LiveTool(n)=>lab.tools.live&&lab.tools.live_tool==n,
        LabAction::LivePoint=>lab.tools.point,
        LabAction::SurfaceSnap=>c.workspace.surface_snap.unwrap_or(true),
        LabAction::AllowOverlap=>c.workspace.allow_overlap,
        LabAction::AddPart => c.mode == constructor::Mode::Part,
        LabAction::Aim => c.mode == constructor::Mode::Aim,
        LabAction::PickCatalog(i) => c.choice == i && c.mode == constructor::Mode::Place,
        LabAction::PickObject(i) => c.selected == Some(i),
        LabAction::PickMaterial(id) => c.material(&lab.definition,lab.recipe)==id,
        LabAction::Category(i) => lab.ui.category == i,
        LabAction::Tab(i) => lab.ui.tab == i,
        LabAction::Grid => lab.ui.grid,
        LabAction::SectionToggle => lab.tools.section.enabled,
        LabAction::Help => lab.ui.help,
        LabAction::ParameterDetails => lab.ui.parameter_details,
        LabAction::Experiments => lab.browser,
        LabAction::Stand => c.stand,
        LabAction::Effect(i) => sources::active(lab).map_or(
            c.weapon.is_none()
                && experiments::weapon_label(lab).is_none()
                && i == lab.recipe.effect,
            |s| s.effect == Some(i),
        ),
        _ => false,
    }
}
pub fn text(lab: &Laboratory, kind: u8) -> String {
    let c = &lab.constructor;
    match kind {
        27=>format!("{} · точка X {} Y {} Z {} · сдвиг {:+} см",if lab.tools.section.enabled {"Разрез включён"}else{"Разрез выключен"},lab.tools.section.anchor[0],lab.tools.section.anchor[1],lab.tools.section.anchor[2],lab.tools.section.offset.saturating_mul(10)),
        26=>format!("Выбрано: {}",scenario::label(lab,lab.ui.scenario_input)),
        12=>{
            if let Some((_,text,_))=&lab.ui.editing {return format!("Ввод: {text} · Enter — применить · Esc — отмена");}
            if c.mode==constructor::Mode::Event {return format!("{} · ЛКМ — поставить · PgUp/PgDn — высота · Esc — отмена · затем F — пуск",lab.ui.event.and_then(|i|lab.events.get(i)).map_or("Воздействие",|e|e.name.as_str()));}
            if c.mode==constructor::Mode::Source {return "Размещение точки оружия: ЛКМ — задать источник · PgUp / PgDn — высота · Esc — отмена".into();}
            if matches!(c.mode,constructor::Mode::Place|constructor::Mode::Part|constructor::Mode::Move) {
                let state=c.hover.and_then(|_|c.preview.as_ref()).map_or("Наведите на испытательную площадку".into(),|p|p.issue.clone().unwrap_or("Можно разместить · ЛКМ".into()));
                let (turns,elevation)=if c.mode==constructor::Mode::Move {c.selected.and_then(|i|c.placed.get(i)).map_or((c.turns,c.elevation),|e|(e.turns,e.at[1]-lab.definition.layout.ground_level))}else{(c.turns,c.elevation)};
                format!("{}  ·  {}°  ·  Y {:+}  ·  Q/E — поворот, PgUp/PgDn — высота, ПКМ — отмена",state,turns as u16*90,elevation)
            } else if c.mode==constructor::Mode::Aim {
                "Выбор цели: нажмите на поверхность или объект · ЛКМ — задать цель · ПКМ / Esc — отмена".into()
            } else {format!("{}  ·  V выбор · G перенос · B деталь · T цель · F опыт · F1 помощь",lab.notice)}
        },
        13=>match lab.ui.category {7=>format!("{} воздействий · страница {} · Клик по карточке → место в сцене → F · Мощность у каждого источника своя",lab.events.len(),lab.ui.page+1),8=>format!("{} готовых физических сценариев · страница {} · Карточка открывает отдельную расстановку · F — запуск",lab.experiments.len(),lab.ui.page+1),_=>format!("{} объектов в категории · страница {} · Клик по карточке → размещение в сцене · Шаг привязки {} ед. · {}",filtered(lab).len(),lab.ui.page+1,lab.definition.editor_step,c.catalog[c.choice].name)},
        15=>if filtered(lab).is_empty(){"В этой категории пока нет объектов. Выберите другую категорию или создайте свой элемент кнопкой «Новый элемент» слева.".into()}else{String::new()},
        18=>if c.creating(){"Создание элемента: добавьте детали и сохраните сборку в «Мои элементы».".into()}else{format!("Объектов: {} · {}",c.placed.len(),if c.stand{"стенд включён"}else{"свободная сцена"})},
        _=>String::new()
    }
}

fn icon_bytes(name: &str) -> &'static [u8] {
    macro_rules! icons {($($n:literal),*)=>{match name {$($n=>include_bytes!(concat!("../../../../../../data/ui/icons/",$n,".rgba")),)*_=>include_bytes!("../../../../../../data/ui/icons/box.rgba")}};}
    icons!(
        "mouse-pointer-2",
        "move",
        "rotate-cw",
        "rotate-ccw",
        "box",
        "plus",
        "trash-2",
        "copy",
        "crosshair",
        "pipette",
        "play",
        "pause",
        "step-forward",
        "undo-2",
        "save",
        "folder-open",
        "flask-conical",
        "grid-3x3",
        "layers",
        "chevron-left",
        "chevron-right",
        "x",
        "circle-help",
        "hammer",
        "tree-pine",
        "shield",
        "settings-2",
        "file-text",
        "camera",
        "search",
        "flame",
        "zap",
        "radiation",
        "sparkles",
        "bomb"
    )
}
fn event_image(pictures:&mut Pictures,images:&mut Assets<Image>,name:&'static str)->Handle<Image> {
    if let Some((_,h))=pictures.event_icons.iter().find(|(n,_)|*n==name) {return h.clone();}
    macro_rules! large {($($n:literal),*)=>{match name {$($n=>include_bytes!(concat!("../../../../../../data/ui/icons/large/",$n,".rgba")).as_slice(),)*_=>include_bytes!("../../../../../../data/ui/icons/large/bomb.rgba").as_slice()}};}
    let bytes=large!("bomb","radiation","hammer","zap","flame","sparkles","crosshair","layers");
    let h=images.add(Image::new(Extent3d {width:192,height:192,depth_or_array_layers:1},TextureDimension::D2,bytes.to_vec(),TextureFormat::Rgba8UnormSrgb,RenderAssetUsages::default()));
    pictures.event_icons.push((name,h.clone()));h
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn sync(
    mut commands: Commands,
    mut lab: ResMut<Laboratory>,
    windows: Query<&Window>,
    (mut scale, mut tones): (ResMut<UiScale>, Query<(&ControlText, &mut TextColor)>),
    templates: Option<Res<workbench::ghost::Templates>>,
    pictures: Option<ResMut<Pictures>>,
    mut images: ResMut<Assets<Image>>,
    mut icons: Query<
        (&Icon, &mut ImageNode),
        (Without<CardImage>, Without<Inspector>, Without<Help>),
    >,
    mut cards: Query<
        (&Card, &mut Node, &mut LabAction, &mut BorderColor),
        (Without<Object>, Without<Inspector>, Without<Help>),
    >,
    mut card_images: Query<(&CardImage, &mut ImageNode, &mut Node), (Without<Icon>, Without<Card>, Without<Object>, Without<Inspector>, Without<Help>)>,
    mut card_names: Query<(&CardName, &mut Text), Without<ObjectName>>,
    mut objects: Query<
        (&Object, &mut Node, &mut BorderColor),
        (Without<Card>, Without<Inspector>, Without<Help>),
    >,
    mut object_names: Query<(&ObjectName, &mut Text), Without<CardName>>,
    mut tabs: Query<(&Inspector, &mut Node), (Without<Card>, Without<Object>, Without<Help>)>,
    mut help: Query<
        &mut Node,
        (
            With<Help>,
            Without<Card>,
            Without<Object>,
            Without<Inspector>,
        ),
    >,
    (mut buttons, hovers): (Query<(Entity, &LabAction, Has<bevy::ui::InteractionDisabled>), Without<Card>>, Query<(&Card,&Interaction)>),
) {
    if !lab.active() {
        if let Some(previous) = lab.ui.scale.take() {
            scale.0 = previous;
        }
        return;
    }
    let Some(mut pictures) = pictures else {
        return;
    };
    if lab.ui.scale.is_none() {
        lab.ui.scale = Some(scale.0);
    }
    if let Ok(window) = windows.single() {
        let value = (window.width() / 1920.0).clamp(1.0, 1.75);
        if scale.0 != value {
            scale.0 = value;
        }
    }
    for (ControlText(action), mut colour) in &mut tones {
        let value = if enabled(&lab, *action) {
            theme::TEXT
        } else {
            Color::srgba(0.40, 0.48, 0.56, 0.7)
        };
        if colour.0 != value {
            colour.0 = value;
        }
    }
    for (entity, action, disabled) in &mut buttons {
        let is_disabled = !enabled(&lab, *action);
        if is_disabled != disabled {
            if is_disabled {
                commands
                    .entity(entity)
                    .insert(bevy::ui::InteractionDisabled);
            } else {
                commands
                    .entity(entity)
                    .remove::<bevy::ui::InteractionDisabled>();
            }
        }
    }
    for (Icon(name), mut image) in &mut icons {
        let handle = if let Some((_, h)) = pictures.icons.iter().find(|(n, _)| n == name) {
            h.clone()
        } else {
            let h = images.add(Image::new(
                Extent3d {
                    width: 40,
                    height: 40,
                    depth_or_array_layers: 1,
                },
                TextureDimension::D2,
                icon_bytes(name).to_vec(),
                TextureFormat::Rgba8UnormSrgb,
                RenderAssetUsages::default(),
            ));
            pictures.icons.push((name, h.clone()));
            h
        };
        if image.image != handle {
            image.image = handle;
        }
    }
    let count = windows
        .single()
        .map_or(8, |w| ((w.width() / scale.0 - 48.0) / 153.0) as usize)
        .clamp(1, SLOTS);
    lab.ui.per_page = count;
    let list = filtered(&lab);
    lab.ui.page = lab.ui.page.min(list.len().saturating_sub(1) / count);
    let page = &list[(lab.ui.page * count).min(list.len())..];
    if let Some(i)=hovers.iter().find(|(_,interaction)|**interaction==Interaction::Hovered).and_then(|(Card(slot),_)|page.get(*slot)) {
        let description=match lab.ui.category {7=>Some(lab.events[*i].description.clone()),8=>Some(lab.experiments[*i].plan.description.clone()),_=>None};
        if let Some(text)=description {if lab.notice!=text {lab.notice=text;}}
    }
    // Only one missing asset thumbnail per frame; reuse exact cached template geometry.
    let mut missing = false;
    if let Some(templates) = templates.as_ref().filter(|_|lab.ui.category<7) {
        if let Some(element) = page
            .iter()
            .take(count)
            .map(|i| &lab.constructor.catalog[*i])
            .find(|e| !pictures.assets.iter().any(|(cached, _)| cached == *e))
        {
            missing = true;
            let h = images.add(visuals::thumbnail(element, &templates));
            pictures.assets.push((element.clone(), h));
        }
    }
    if lab.ui.category==8 {
        if let Some(templates)=templates.as_ref() {
            if let Some(i)=page.iter().take(count).find(|i|!pictures.scenarios.iter().any(|(cached,_)|cached==*i)) {
                missing=true;
                let e=&lab.experiments[*i];
                let element=Element {id:e.id.clone(),name:e.plan.name.clone(),category:Category::Custom,
                    blocks:e.layout.blocks.clone(),units:e.layout.units.clone(),unit_yaws:e.layout.unit_yaws.clone(),shields:e.layout.shields.clone()};
                let h=if element.blocks.is_empty()&&element.units.is_empty()&&element.shields.is_empty() {event_image(&mut pictures,&mut images,"layers")}else{images.add(visuals::thumbnail(&element,templates))};
                pictures.scenarios.push((*i,h));
            }
        }
    }
    lab.ui.ready_frames = if missing {
        0
    } else {
        lab.ui.ready_frames.saturating_add(1)
    };
    for (Card(slot), mut node, mut action, mut border) in &mut cards {
        let i = page.get(*slot).filter(|_| *slot < count).copied();
        display(
            &mut node,
            if i.is_some() {
                Display::Flex
            } else {
                Display::None
            },
        );
        if let Some(i) = i {
            let value=match lab.ui.category {7=>LabAction::PickEvent(i),8=>LabAction::Experiment(i),_=>LabAction::PickCatalog(i)};
            if *action != value {
                *action = value;
            }
            *border = BorderColor::all(if selected(&lab, *action) {
                theme::ACCENT
            } else {
                theme::FRAME
            });
        }
    }
    for (CardImage(slot), mut image, mut node) in &mut card_images {
        let height=px(if lab.ui.category==8 {74}else{94});
        if node.height!=height {node.height=height;}
        let empty=lab.ui.category==8&&page.get(*slot).is_some_and(|i| {let e=&lab.experiments[*i];e.layout.blocks.is_empty()&&e.layout.units.is_empty()&&e.layout.shields.is_empty()});
        let width=match lab.ui.category {7=>px(94),8=>px(if empty {74}else{106}),_=>percent(100)};
        if node.width!=width {node.width=width;}
        let alignment=if lab.ui.category>=7 {AlignSelf::Center}else{AlignSelf::Auto};
        if node.align_self!=alignment {node.align_self=alignment;}
        let handle = if lab.ui.category==7 {
            page.get(*slot).map(|i| {
                let name=events::icon(&lab.events[*i].icon);
                event_image(&mut pictures,&mut images,name)
            })
        }else if lab.ui.category==8 {
            page.get(*slot).and_then(|i|pictures.scenarios.iter().find(|(cached,_)|cached==i).map(|(_,h)|h.clone()))
        }else{page
            .get(*slot)
            .and_then(|i| {
                pictures
                    .assets
                    .iter()
                    .find(|(e, _)| e == &lab.constructor.catalog[*i])
            })
            .map(|(_, h)| h.clone())};
        if let Some(handle) = handle {
            if image.image != handle {
                image.image = handle;
            }
            if image.color != Color::WHITE {
                image.color = Color::WHITE;
            }
        } else if image.color != Color::NONE {
            image.color = Color::NONE;
        }
    }
    for (CardName(slot), mut text) in &mut card_names {
        let value = page
            .get(*slot)
            .map_or("", |i| match lab.ui.category {7=>lab.events[*i].name.as_str(),8=>lab.experiments[*i].plan.name.as_str(),_=>lab.constructor.catalog[*i].name.as_str()});
        if text.0 != value {
            **text = value.to_string();
        }
    }
    for (Object(i), mut node, mut border) in &mut objects {
        display(
            &mut node,
            if lab.constructor.placed.get(*i).is_some() {
                Display::Flex
            } else {
                Display::None
            },
        );
        *border = BorderColor::all(if lab.constructor.selected == Some(*i) {
            theme::ACCENT
        } else {
            theme::FRAME
        });
    }
    for (ObjectName(i), mut text) in &mut object_names {
        let value = lab
            .constructor
            .placed
            .get(*i)
            .map_or(String::new(), |e| format!("{} · {}", i + 1, e.element.name));
        if text.0 != value {
            **text = value;
        }
    }
    for (Inspector(i), mut node) in &mut tabs {
        display(
            &mut node,
            if section_visible(&lab, *i) {
                Display::Flex
            } else {
                Display::None
            },
        );
    }
    for mut node in &mut help {
        display(
            &mut node,
            if lab.ui.help {
                Display::Flex
            } else {
                Display::None
            },
        );
    }
}

pub fn capture_ready(lab: &Laboratory) -> bool {
    !lab.active() || (!lab.tools.flight.profile && lab.ui.ready_frames >= 4 && lab.ui.viewport_ready >= 4)
}
/// Explicit screenshot fixtures only; never used by a normal interactive session.
pub fn capture_fixture(lab: &mut Laboratory) -> bool {
    let Ok(case) = std::env::var("RTS_LAB_CAPTURE_CASE") else {
        return false;
    };
    if case == "melt-steel" {
        let c=&mut lab.constructor;
        c.placed.clear(); c.selected=None; c.base=None; c.stand=false;
        c.placed.push(modding::laboratory::catalog::Placed {
            element:modding::laboratory::catalog::Element {
                id:"melt-capture".into(), name:"Стальная плита".into(), category:Category::Custom,
                blocks:vec![RangeBlock{min:[0;3],max:[4,30,30],material:3}],
                units:vec![], unit_yaws:vec![], shields:vec![],
            }, at:[384,16,330], turns:0,
        });
        let centre=Vec3::new(384.0,31.0,345.0);
        lab.tools.flying=true;
        lab.tools.camera=Some(Transform::from_translation(centre+Vec3::new(-52.0,38.0,62.0)).looking_at(centre,Vec3::Y));
        lab.ui.tab=3;
        return true;
    }
    if ["collector-before","collector-after","collector-profile"].contains(&case.as_str()) {
        let index=lab.experiments.iter().position(|e|e.id=="crawler-salvage").unwrap();
        experiments::select(lab,index).expect("collector capture fixture");lab.ui.tab=3;return true;
    }
    if ["dune-before","dune-during","dune-after","dune-profile"].contains(&case.as_str()) {
        let index=lab.experiments.iter().position(|e|e.id=="dune-rockfall").unwrap();
        experiments::select(lab,index).expect("dune capture fixture");lab.ui.tab=3;return true;
    }
    if ["water-gravity-before","water-gravity-during","water-gravity-after","water-gravity-profile"].contains(&case.as_str()) {
        let index=lab.experiments.iter().position(|e|e.id=="water-fall").unwrap();
        experiments::select(lab,index).expect("water gravity fixture");lab.ui.tab=3;return true;
    }
    if ![
        "overview", "valid", "invalid", "help", "results", "weapon", "draft", "block", "aim",
        "sources", "settings", "number", "bound", "error", "events", "scenarios", "collapse-kit", "event-preview", "material-scene", "material-catalog", "material-result", "water-scene", "water-result", "water-float", "physics-tools", "physics-wetting", "physics-dome", "physics-devices", "physics-glass", "physics-erosion", "material-object", "material-stand", "voxel-inspector", "voxel-editor", "live-tools", "he-fragments-flight", "he-fragments-landed", "fragment-inspector", "surround", "fps-laser", "fps-water", "fps-menu", "fps-profile", "fps-water-profile", "fps-fire", "fps-fire-profile", "fps-mg-profile", "fps-he-profile", "fps-ap-profile",
    ]
    .contains(&case.as_str())
    {
        return false;
    }
    let c = &mut lab.constructor;
    c.stand = false;
    c.placed.clear();
    for (id, at) in [
        ("tank", [272, 16, 320]),
        ("bunker", [384, 16, 400]),
        ("tree", [416, 16, 288]),
    ] {
        if let Some(element) = c.catalog.iter().find(|e| e.id == id).cloned() {
            c.placed.push(modding::laboratory::catalog::Placed {
                element,
                at,
                turns: 0,
            });
        }
    }
    c.selected = Some(0);
    c.revision += 1;
    if case=="fragment-inspector" {
        c.placed.clear();c.selected=None;c.base=None;c.stand=false;c.mode=constructor::Mode::Inspect;
        lab.ui.tab=14;return true;
    }
    if case.starts_with("he-fragments-") {
        c.placed.clear();c.selected=None;c.base=None;c.stand=false;
        c.placed.push(modding::laboratory::catalog::Placed {
            element:modding::laboratory::catalog::Element {
                id:"he-fragments-check".into(),name:"Бетонная мишень".into(),category:Category::Custom,
                blocks:vec![RangeBlock{min:[0;3],max:[16,12,16],material:2}],units:vec![],unit_yaws:vec![],shields:vec![],
            },at:[384,16,336],turns:0,
        });
        let centre=Vec3::new(392.0,18.0,344.0);
        lab.tools.flying=true;lab.tools.camera=Some(Transform::from_translation(centre+Vec3::new(48.0,38.0,50.0)).looking_at(centre,Vec3::Y));
        lab.ui.tab=3;
        return true;
    }
    if case == "block" {
        c.placed[0].element = c
            .catalog
            .iter()
            .find(|e| e.blocks.len() == 1 && e.units.is_empty() && e.shields.is_empty())
            .unwrap()
            .clone();
    }
    if case == "draft" {
        c.action(LabAction::Create, &lab.definition, &mut lab.recipe)
            .unwrap();
        c.mode = constructor::Mode::Part;
        lab.ui.capture_at = Some([384, 16, 320]);
    }
    if case == "aim" {
        c.mode = constructor::Mode::Aim;
    }
    if case == "valid" || case == "invalid" {
        c.choice = c.catalog.iter().position(|e| e.id == "tank").unwrap();
        c.mode = constructor::Mode::Place;
        c.turns = 1;
        lab.ui.capture_at = Some(if case == "valid" {
            [384, 16, 208]
        } else {
            [272, 16, 320]
        });
    }
    if case=="material-object" || case=="material-stand" {
        if case=="material-object" {c.action(LabAction::PickObject(1),&lab.definition,&mut lab.recipe).unwrap();}
        else {c.placed.clear();c.selected=None;c.stand=true;c.base=None;c.mode=constructor::Mode::Select;}
        c.action(LabAction::PickMaterial(5),&lab.definition,&mut lab.recipe).unwrap();
        if case=="material-object" {c.action(LabAction::ApplyMaterial,&lab.definition,&mut lab.recipe).unwrap();}
        lab.ui.tab=1;
    }
    if case == "voxel-editor" {
        c.selected=Some(1);c.action(LabAction::BeginVoxelEdit,&lab.definition,&mut lab.recipe).unwrap();
        let item=&c.placed[1];let b=item.element.blocks.iter().find(|b|b.material!=0).unwrap();
        let at=modding::laboratory::catalog::voxels::world_cell(item,b.min);
        for (p,tool,mat) in [(at,modding::laboratory::catalog::voxels::Tool::Paint,3),([at[0],at[1]+1,at[2]],modding::laboratory::catalog::voxels::Tool::Erase,3),([at[0]-1,at[1],at[2]],modding::laboratory::catalog::voxels::Tool::Add,5)] {
            c.stage_voxel_cells(std::collections::BTreeSet::from([p]),tool,mat).unwrap();
            c.action(LabAction::VoxelStroke,&lab.definition,&mut lab.recipe).unwrap();
        }
        lab.tools.selected=Some(at);lab.tools.probe_direction=Some(Vec3::new(6.0,-5.0,6.0).normalize());
        let centre=Vec3::from(at.map(|v|v as f32))+Vec3::splat(0.5);
        lab.tools.camera=Some(Transform::from_translation(centre+Vec3::new(-6.0,5.0,-6.0).normalize()*10.0).looking_at(centre,Vec3::Y));lab.tools.flying=true;lab.ui.tab=17;
    }
    if case == "voxel-inspector" {
        c.selected=None;c.mode=constructor::Mode::Inspect;lab.ui.tab=14;
        let item=&c.placed[1];let block=item.element.blocks.iter().find(|b|b.material!=0).unwrap();
        lab.tools.selected=Some(std::array::from_fn(|a|item.at[a]+block.min[a]));
        lab.tools.probe_direction=Some(Vec3::new(6.0,-5.0,6.0).normalize());
        tools::inspector::focus(lab);
        lab.tools.probe_stamp=None;
    }
    lab.ui.help = case == "help";
    if case == "weapon" {
        lab.ui.tab = 2;
    }
    if case == "results" {
        lab.ui.tab = 3;
    }
    if ["sources", "settings", "number", "bound", "error"].contains(&case.as_str()) {
        sources::ensure(lab).unwrap();
        let mut second = sources::active(lab).unwrap().clone();
        second.name = "Источник 2".into();
        second.at = [320, 72, 256];
        second.power_milli = 2750;
        lab.constructor.workspace.sources.push(second);
        lab.constructor.workspace.selected = 1;
        lab.ui.tab = if case == "settings" || case == "error" {
            9
        } else if case == "number" {
            2
        } else {
            10
        };
        if case == "number" {
            // Exercise replacement at the input length limit in native captures.
            lab.ui.editing = Some((0, "11111111111111111111".into(), false));
            let mut keys = ButtonInput::<KeyCode>::default();
            keys.press(KeyCode::ControlLeft);
            keys.press(KeyCode::KeyA);
            parameters::keyboard(lab, &mut keys);
            for key in [KeyCode::Digit2, KeyCode::Period, KeyCode::Digit7, KeyCode::Digit5] {
                keys.press(key);
                parameters::keyboard(lab, &mut keys);
            }
        }
        if case == "error" {
            lab.ui.editing = Some((20, "1001".into(), false));
            lab.ui.edit_error = Some("Размер: кратный 16, стороны 256–2048, высота 128–512".into());
        }
        if case == "bound" {
            sources::active_mut(lab).unwrap().binding = Some(0);
        }
    }
    if case=="events" || case=="event-preview" {lab.ui.category=7;}
    if case=="scenarios" {lab.ui.category=8;}
    if case=="material-scene" || case=="material-catalog" || case=="material-result" {
        let index=lab.experiments.iter().position(|e|e.id=="rock-sand").unwrap();
        experiments::select(lab,index).unwrap();
        lab.ui.category=8; lab.ui.page=2; lab.ui.tab=3;
    }
    if case=="water-scene" || case=="water-result" || case=="water-float" {
        let id=if case=="water-float" {"water-float"}else{"water-fall"};
        let index=lab.experiments.iter().position(|e|e.id==id).unwrap();
        experiments::select(lab,index).unwrap();
        lab.ui.category=8; lab.ui.page=lab.experiments.len().saturating_sub(1)/8;lab.ui.tab=3;
    }
    if case.starts_with("physics-") {
        let id=match case.as_str(){"physics-dome"=>"buried-dome","physics-devices"=>"drill-crawler","physics-glass"=>"vitrification","physics-erosion"=>"water-erosion",_=>"wetting"};
        let index=lab.experiments.iter().position(|e|e.id==id).unwrap();
        experiments::select(lab,index).unwrap();lab.ui.category=8;lab.ui.page=lab.experiments.len().saturating_sub(1)/8;lab.ui.tab=if case=="physics-tools"{16}else{3};
    }
    if case=="collapse-kit" || case=="event-preview" {
        lab.constructor.placed.clear();
        let element=lab.constructor.catalog.iter().find(|e|e.id=="collapse-tower").unwrap().clone();
        lab.constructor.placed.push(modding::laboratory::catalog::Placed{element,at:[384,16,336],turns:0});
        sources::ensure(lab).unwrap();
        let source=sources::active_mut(lab).unwrap();
        source.name="Подрыв опоры".into();source.detonate=true;source.at=[384,28,336];source.target=source.at;source.power_milli=500;
        lab.ui.tab=2;
        lab.ui.category=7;
        if case=="event-preview" {lab.constructor.mode=constructor::Mode::Event;lab.ui.event=Some(4);lab.ui.capture_at=Some([384,28,336]);}
    }
    true
}

pub fn enabled(lab: &Laboratory, a: LabAction) -> bool {
    if !lab.constructor.voxel_allowed(a) {return false;}
    if !live_tools::allowed(lab,a) {return false;}
    if a==LabAction::BeginVoxelEdit {return lab.constructor.can_voxel_edit();}
    let c = &lab.constructor;
    match a {
        LabAction::LiveTool(_)|LabAction::LivePoint|LabAction::LivePower(_)|LabAction::LiveCapture|LabAction::LiveSpeed(_)|LabAction::LiveLook(_)|LabAction::LiveFov(_)|LabAction::Tab(21)=>lab.tools.live,
        LabAction::VoxelUndo=>c.voxel_can_undo(false),
        LabAction::VoxelRedo=>c.voxel_can_undo(true),
        LabAction::VoxelTool(_)|LabAction::VoxelFinish(_)=>c.voxel_editing(),
        LabAction::SavePart=>c.selected.is_some(),
        LabAction::Tab(17)=>c.voxel_editing()||c.can_voxel_edit(),
        LabAction::ReturnConstruction=>c.can_return(),
        LabAction::PickEvent(i) | LabAction::PlaceEvent(i,_)=>i<lab.events.len()&&c.workspace.sources.len()<32,
        LabAction::LocalCharge(local)=>sources::active(lab).is_some_and(|s|!local||s.config.delivery==Delivery::Blast),
        LabAction::ApplyNumber | LabAction::CancelNumber => lab.ui.editing.is_some(),
        LabAction::EditNumber(id) | LabAction::AdjustNumber(id, _) if matches!(id,0..=15|40..=54) => {
            sources::active(lab).is_some()
        }
        LabAction::AddSource => c.workspace.sources.len() < 32 && !c.base.as_ref().is_some_and(|b|b.battle),
        LabAction::Source(i) => i < c.workspace.sources.len(),
        LabAction::RemoveSource => !c.workspace.sources.is_empty() && (c.experiment.is_some() || c.workspace.sources.len()>1),
        LabAction::BindSource(true) => {
            c.selected
                .is_some_and(|i| !c.placed[i].element.units.is_empty())
                && sources::active(lab).is_some()
        }
        LabAction::SourcePosition | LabAction::BindSource(false) => sources::active(lab).is_some(),
        LabAction::Delete | LabAction::Duplicate | LabAction::MoveAt(_) => editing(lab).is_some(),
        LabAction::PickObject(i) => i < c.placed.len(),
        LabAction::Form(_) | LabAction::Thickness | LabAction::Strength => c.stand && c.base.is_none(),
        LabAction::ApplyMaterial => c.can_apply_material(),
        LabAction::ApplySize => single_block(lab),
        LabAction::Split => {
            editing(lab).is_some_and(|e| e.blocks.len() + e.units.len() + e.shields.len() > 1)
        }
        LabAction::Move(_, _) => {
            c.selected.is_some()
                && !matches!(c.mode, constructor::Mode::Place | constructor::Mode::Part | constructor::Mode::Drill | constructor::Mode::Crawler | constructor::Mode::Collector)
        }
        LabAction::Tool(1) => c.selected.is_some(),
        LabAction::Turn(_) => {
            c.selected.is_some()
                || matches!(c.mode, constructor::Mode::Place | constructor::Mode::Part | constructor::Mode::Drill | constructor::Mode::Crawler | constructor::Mode::Collector)
        }
        LabAction::Elevation(_) => {
            c.selected.is_some()
                || matches!(
                    c.mode,
                    constructor::Mode::Place | constructor::Mode::Part | constructor::Mode::Source | constructor::Mode::Event | constructor::Mode::Brush | constructor::Mode::Erase | constructor::Mode::Drill | constructor::Mode::Crawler | constructor::Mode::Collector
                )
        }
        LabAction::SaveElement => c.creating() && !c.placed.is_empty(),
        LabAction::CancelCreate => c.creating(),
        LabAction::Create => !c.creating(),
        LabAction::Page(delta) => {
            if delta < 0 {
                lab.ui.page > 0
            } else {
                lab.ui.page.saturating_add(1).saturating_mul(lab.ui.per_page) < filtered(lab).len()
            }
        }
        _ => true,
    }
}

fn display(node: &mut Mut<'_, Node>, value: Display) {
    if node.display != value {
        node.display = value;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scenario_and_device_panels_show_only_applicable_fields() {
        use modding::laboratory::experiments::{Input,Plan};
        let mut lab=Laboratory::new(false);
        lab.constructor.experiment=Some(Plan{name:"Проверка".into(),description:String::new(),duration_ticks:10,
            inputs:vec![Input::FreeControl,Input::Move{actor:0,to:[80,80]},Input::Pilot{actor:0,throttle:1,turn:0,aim:[80,16,80],fire:false}]});
        lab.ui.tab=24;
        assert!(section_visible(&lab,24));assert!(!section_visible(&lab,25));assert!(!section_visible(&lab,22));
        assert!(!section_visible(&lab,26));assert!(!section_visible(&lab,27));
        lab.ui.scenario_input=1;
        assert!(section_visible(&lab,26));assert!(section_visible(&lab,27));assert!(!section_visible(&lab,28));
        lab.ui.scenario_input=2;assert!(section_visible(&lab,28));
        lab.ui.tab=25;assert!(section_visible(&lab,25));assert!(!section_visible(&lab,24));
        assert!(!section_visible(&lab,29));
    }
    #[test]
    fn laboratory_docks_filter_cards_and_switch_tabs_without_rebuilding_the_world() {
        let mut app = super::super::integration_tests::app();
        app.init_resource::<UiScale>()
            .init_resource::<Assets<Image>>()
            .add_systems(Startup, |mut commands: Commands| {
                let camera = commands.spawn_empty().id();
                build(&mut commands, camera, &TextFont::default());
            })
            .add_systems(
                Update,
                (super::super::refresh, sync, parameters::sync)
                    .chain()
                    .after(super::super::actions),
            );
        super::super::integration_tests::enter(&mut app);
        assert_eq!(
            app.world()
                .resource::<Laboratory>()
                .viewport_margins(Vec2::new(3840.0, 2160.0)),
            Some([234.5, 556.5, 458.5, 619.5])
        );
        let before = app.world().resource::<Scene>().sim.snapshot();
        super::super::integration_tests::click(&mut app, LabAction::Category(3));
        let world = app.world_mut();
        let mut cards = world.query::<(&Card, &Node, &LabAction)>();
        let visible: Vec<_> = cards
            .iter(world)
            .filter(|(_, n, _)| n.display == Display::Flex)
            .collect();
        assert_eq!(visible.len(), 3);
        for (_, _, action) in visible {
            let LabAction::PickCatalog(i) = action else {
                panic!("card is not actionable")
            };
            assert_eq!(
                world.resource::<Laboratory>().constructor.catalog[*i].category,
                Category::Unit
            );
        }
        super::super::integration_tests::click(&mut app, LabAction::Tab(2));
        let world = app.world_mut();
        let mut tabs = world.query::<(&Inspector, &Node)>();
        assert!(tabs
            .iter(world)
            .filter(|(i, _)| i.0 < 4)
            .all(|(i, n)| (n.display == Display::Flex) == (i.0 == 2)));
        for category in [7,8] {
            super::super::integration_tests::click(&mut app,LabAction::Category(category));
            let world=app.world_mut();
            let mut cards=world.query::<(&Card,&Node,&LabAction)>();
            let actions:Vec<_>=cards.iter(world).filter(|(_,n,_)|n.display==Display::Flex).map(|(_,_,a)|*a).collect();
            assert!(!actions.is_empty());
            assert!(actions.iter().all(|a|if category==7 {matches!(a,LabAction::PickEvent(_))}else{matches!(a,LabAction::Experiment(_))}));
        }
        assert_eq!(
            app.world().resource::<Scene>().sim.snapshot(),
            before,
            "UI browsing must not reset or tick SIM"
        );
    }
    #[test]
    fn laboratory_empty_custom_category_is_safe_and_disabled_actions_are_explicit() {
        let mut lab = Laboratory::new(false);
        lab.constructor
            .catalog
            .retain(|e| e.category != Category::Custom);
        lab.ui.category = 6;
        assert!(filtered(&lab).is_empty());
        assert!(!enabled(&lab, LabAction::Delete));
        assert!(!enabled(&lab, LabAction::Tool(1)));
        assert!(!enabled(&lab, LabAction::SaveElement));
        assert!(!enabled(&lab, LabAction::Page(1)));
        assert!(text(&lab, 13).contains("0 объектов"));
    }

    #[test]
    fn inspector_actions_follow_object_type_and_active_placement() {
        let mut lab = Laboratory::new(false);
        let tank = lab
            .constructor
            .catalog
            .iter()
            .find(|e| e.id == "tank")
            .unwrap()
            .clone();
        lab.constructor
            .placed
            .push(modding::laboratory::catalog::Placed {
                element: tank,
                at: [272, 16, 320],
                turns: 0,
            });
        lab.constructor.selected = Some(0);
        assert!(enabled(&lab, LabAction::Delete));
        assert!(!enabled(&lab, LabAction::ApplySize));
        assert!(!enabled(&lab, LabAction::ApplyMaterial));
        assert!(!enabled(&lab, LabAction::Split));
        assert!(!section_visible(&lab, 7));
        lab.constructor.placed[0].element = lab
            .constructor
            .catalog
            .iter()
            .find(|e| e.blocks.len() == 1 && e.units.is_empty() && e.shields.is_empty())
            .unwrap()
            .clone();
        assert!(enabled(&lab, LabAction::ApplySize));
        assert!(section_visible(&lab, 7));
        lab.constructor.mode = constructor::Mode::Place;
        for action in [
            LabAction::Delete,
            LabAction::Duplicate,
            LabAction::ApplyMaterial,
            LabAction::ApplySize,
            LabAction::Move(0, 1),
        ] {
            assert!(
                !enabled(&lab, action),
                "placement cannot edit stale selection"
            );
        }
        assert!(!section_visible(&lab, 5));
        assert!(section_visible(&lab, 6));
        assert!(!enabled(&lab, LabAction::PickObject(99)));
        lab.constructor.mode = constructor::Mode::Aim;
        assert!(text(&lab, 12).contains("Выбор цели"));
    }
}

impl Laboratory {
    pub fn viewport_margins(&self, viewport: Vec2) -> Option<[f32; 4]> {
        (self.active() && !first_person::immersive(self)).then(|| {
            let factor = (viewport.x / 1920.0).clamp(1.0, 1.75);
            [134.0, 318.0, 262.0, 354.0].map(|v| v * factor)
        })
    }
}

#[cfg(test)]
#[test]
fn voxel_editor_sections_and_history_controls_are_visible_for_a_single_block() {
    let mut lab=Laboratory::new(false);lab.constructor.stand=false;
    let element=lab.constructor.catalog.iter().find(|e|e.blocks.len()==1&&e.units.is_empty()&&e.shields.is_empty()).unwrap().clone();
    lab.constructor.placed=vec![modding::laboratory::catalog::Placed{element,at:[384,16,336],turns:0}];lab.constructor.selected=Some(0);lab.ui.tab=0;
    assert!(section_visible(&lab,18));assert!(enabled(&lab,LabAction::BeginVoxelEdit));
    lab.constructor.action(LabAction::BeginVoxelEdit,&lab.definition,&mut lab.recipe).unwrap();lab.ui.tab=17;
    assert!(section_visible(&lab,17));assert!(enabled(&lab,LabAction::VoxelFinish(true)));
    assert!(!enabled(&lab,LabAction::VoxelUndo));assert!(!enabled(&lab,LabAction::Fire));
    assert!(enabled(&lab,LabAction::Tab(1)));assert!(!enabled(&lab,LabAction::PickObject(0)));
    assert!(lab.constructor.material_context(&lab.definition,lab.recipe).contains("кисти кубиков"));
}
