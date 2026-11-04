//! Local laboratory recording: one initial physical snapshot and ordered inputs.
//! Scrubbing replays in bounded chunks; it never modifies the recorded tape.
use super::*;
use sim_core::blast::battle::commands::{
    runtime::{read_recorded_command, save_recorded_command},
    CommandPhase, Outcome, Producer, Record,
};
use std::io::Write;

const MAX_COMMANDS: usize = 16_384;
const MAX_SNAPSHOT: usize = 128 * 1024 * 1024;
const STEPS_PER_FRAME: usize = 4;
const MAX_FILE: usize = 192 * 1024 * 1024;
// Seek checkpoints are only a transient acceleration, never part of the file.
const CACHE_BYTES: usize = 48 * 1024 * 1024;
const CACHE_COUNT: usize = 3;
const CACHE_INTERVAL: u32 = 240;
const MAGIC: &[u8; 8] = b"RTSLAB01";

pub struct Tape {
    snapshot: Vec<u8>,
    config: ExplosionConfig,
    start: u32,
    end: u32,
    records: Vec<Record>,
    end_hash: u64,
}
fn checksum(data: &[u8]) -> u64 {
    data.iter().fold(0xcbf29ce484222325u64, |a, b| {
        (a ^ u64::from(*b)).wrapping_mul(0x100000001b3)
    })
}
fn outcome_code(o: Outcome) -> u8 {
    match o {
        Outcome::Rejected => 0,
        Outcome::Unchanged => 1,
        Outcome::Applied => 2,
        Outcome::Queued => 3,
        Outcome::PathFailed => 4,
        Outcome::Busy => 5,
    }
}
fn outcome_from(c: u8) -> Option<Outcome> {
    Some(match c {
        0 => Outcome::Rejected,
        1 => Outcome::Unchanged,
        2 => Outcome::Applied,
        3 => Outcome::Queued,
        4 => Outcome::PathFailed,
        5 => Outcome::Busy,
        _ => return None,
    })
}
struct Reader<'a> {
    data: &'a [u8],
    at: usize,
}
impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.at.checked_add(n)?;
        let v = self.data.get(self.at..end)?;
        self.at = end;
        Some(v)
    }
    fn u8(&mut self) -> Option<u8> {
        Some(*self.take(1)?.first()?)
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
    fn command(&mut self) -> Option<Command> {
        let (c, n) = read_recorded_command(self.data.get(self.at..)?)?;
        self.take(n)?;
        Some(c)
    }
}
impl Tape {
    fn encode(&self) -> Vec<u8> {
        let mut b = fx::blob::Blob::new();
        b.v.extend_from_slice(MAGIC);
        b.u32(1);
        b.u32(self.start);
        b.u32(self.end);
        b.u64(self.end_hash);
        save_recorded_command(
            &mut b,
            Command::ConfigureWeapon {
                config: self.config,
            },
        );
        b.u64(self.snapshot.len() as u64);
        b.v.extend_from_slice(&self.snapshot);
        b.u32(self.records.len() as u32);
        for r in &self.records {
            b.u32(r.assigned_tick);
            b.u8(outcome_code(r.outcome));
            save_recorded_command(&mut b, r.command);
        }
        b.u64(checksum(&b.v));
        b.v
    }
    fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() > MAX_FILE || bytes.len() < 36 {
            return None;
        }
        let end = bytes.len().checked_sub(8)?;
        let payload = &bytes[..end];
        if u64::from_le_bytes(bytes[end..].try_into().ok()?) != checksum(payload) {
            return None;
        }
        let mut r = Reader {
            data: payload,
            at: 0,
        };
        if r.take(8)? != MAGIC || r.u32()? != 1 {
            return None;
        }
        let start = r.u32()?;
        let end = r.u32()?;
        let end_hash = r.u64()?;
        if start > end {
            return None;
        }
        let Command::ConfigureWeapon { config } = r.command()? else {
            return None;
        };
        let n = usize::try_from(r.u64()?).ok()?;
        if n > MAX_SNAPSHOT {
            return None;
        }
        let snapshot = r.take(n)?.to_vec();
        let count = r.u32()? as usize;
        if count > MAX_COMMANDS || count > r.data.len().saturating_sub(r.at) / 6 {
            return None;
        }
        let mut records = Vec::with_capacity(count);
        let mut previous = start;
        for sequence in 0..count {
            let tick = r.u32()?;
            if tick < previous || tick > end {
                return None;
            }
            previous = tick;
            let outcome = outcome_from(r.u8()?)?;
            let command = r.command()?;
            records.push(Record {
                requested_tick: tick,
                assigned_tick: tick,
                phase: CommandPhase::BetweenTicks,
                producer: Producer::Player,
                sequence: sequence as u64,
                command,
                outcome,
            });
        }
        if r.at != payload.len() {
            return None;
        }
        let initial = BlastRange::restore(config, &snapshot)?;
        if initial.tick != start {
            return None;
        }
        Some(Self {
            snapshot,
            config,
            start,
            end,
            records,
            end_hash,
        })
    }
}
struct Job {
    sim: BlastRange,
    target: u32,
    next: usize,
    effects: fire::Effects,
}
struct Cached {
    tick: u32,
    next: usize,
    config: ExplosionConfig,
    snapshot: Vec<u8>,
    effects: fire::Effects,
}
#[derive(Default)]
pub struct Replay {
    pub recording: bool,
    pub playing: bool,
    play_next: usize,
    pub tape: Option<Tape>,
    job: Option<Job>,
    cache: Vec<Cached>,
    cache_enabled: bool,
    pub position: Option<u32>,
    pub target: Option<u32>,
    opened: Option<std::path::PathBuf>,
}
impl Replay {
    pub fn observe_tick(&mut self, tick: u32) {
        if self.recording {
            if let Some(t) = &mut self.tape {
                t.end = tick;
            }
        }
    }
    pub fn start(&mut self, sim: &mut BlastRange) -> Result<(), String> {
        if self.recording {
            return Err("Запись уже идёт".into());
        }
        let bytes = sim.snapshot();
        if bytes.len() > MAX_SNAPSHOT {
            return Err("Начальный снимок превышает лимит 128 МиБ".into());
        }
        sim.start_command_capture(MAX_COMMANDS)
            .map_err(str::to_string)?;
        self.cache_enabled = bytes.len() <= CACHE_BYTES;
        self.tape = Some(Tape {
            snapshot: bytes,
            config: sim.config,
            start: sim.tick,
            end: sim.tick,
            records: Vec::new(),
            end_hash: 0,
        });
        self.recording = true;
        self.playing = false;
        self.job = None;
        self.cache.clear();
        self.position = None;
        self.target = None;
        self.opened = None;
        Ok(())
    }
    pub fn stop(&mut self, sim: &mut BlastRange) -> Result<(), String> {
        if !self.recording {
            return Err("Запись не запущена".into());
        }
        self.recording = false;
        self.playing = false;
        let trace = sim
            .finish_command_capture()
            .ok_or("Журнал записи потерян")?;
        if !trace.complete() {
            self.tape = None;
            return Err("Запись превышает лимит команд и не может быть воспроизведена".into());
        }
        let tape = self.tape.as_mut().ok_or("Начальный снимок потерян")?;
        if trace
            .records
            .iter()
            .any(|r| r.producer == Producer::Player && r.phase != CommandPhase::BetweenTicks)
        {
            self.tape = None;
            return Err("Запись содержит неподдерживаемую фазу команды".into());
        }
        tape.end = sim.tick;
        tape.end_hash = sim.state_hash();
        tape.records = trace
            .records
            .into_iter()
            .filter(|r| r.producer == Producer::Player)
            .collect();
        self.position = Some(sim.tick);
        self.target = None;
        self.job = None;
        Ok(())
    }
    pub fn bounds(&self) -> Option<(u32, u32)> {
        self.tape
            .as_ref()
            .filter(|_| !self.recording)
            .map(|t| (t.start, t.end))
    }
    pub fn seek(&mut self, tick: u32) -> Result<(), String> {
        if self.recording {
            return Err("Сначала остановите запись".into());
        }
        self.playing = false;
        let tape = self.tape.as_ref().ok_or("Нет записи")?;
        let target = tick.clamp(tape.start, tape.end);
        if let Some(job) = self.job.as_mut().filter(|j| j.sim.tick <= target) {
            job.target = target;
            self.target = Some(target);
            return Ok(());
        }
        let cached = self
            .cache
            .iter()
            .filter(|c| c.tick <= target)
            .max_by_key(|c| c.tick);
        let (config, snapshot, next, mut effects) = cached.map_or_else(
            || {
                (
                    tape.config,
                    tape.snapshot.as_slice(),
                    0,
                    fire::Effects::default(),
                )
            },
            |c| (c.config, c.snapshot.as_slice(), c.next, c.effects.clone()),
        );
        let mut sim =
            BlastRange::restore(config, snapshot).ok_or("Не удалось восстановить снимок записи")?;
        sim.set_physics_workers(1);
        sim.set_navigation_workers(1);
        effects.before(&sim);
        self.job = Some(Job {
            sim,
            target,
            next,
            effects,
        });
        self.target = Some(target);
        Ok(())
    }
    fn cache_job(&mut self, job: &Job) {
        if !self.cache_enabled || self.cache.iter().any(|c| c.tick == job.sim.tick) {
            return;
        }
        let snapshot = job.sim.snapshot();
        if snapshot.len() > CACHE_BYTES {
            self.cache.clear();
            self.cache_enabled = false;
            return;
        }
        while self.cache.len() >= CACHE_COUNT
            || self.cache.iter().map(|c| c.snapshot.len()).sum::<usize>() + snapshot.len()
                > CACHE_BYTES
        {
            self.cache.remove(0);
        }
        self.cache.push(Cached {
            tick: job.sim.tick,
            next: job.next,
            config: job.sim.config,
            snapshot,
            effects: job.effects.clone(),
        });
    }
    fn apply_at(job: &mut Job, tape: &Tape) -> Result<(), String> {
        while let Some(r) = tape
            .records
            .get(job.next)
            .filter(|r| r.assigned_tick == job.sim.tick)
        {
            let outcome = job.sim.execute_command(r.command);
            if outcome != r.outcome {
                return Err(format!(
                    "Команда на тике {} воспроизведена иначе: {:?} вместо {:?}",
                    job.sim.tick, outcome, r.outcome
                ));
            }
            job.next += 1;
        }
        Ok(())
    }
    pub fn advance(&mut self) -> Result<Option<(BlastRange, fire::Effects)>, String> {
        let Some(mut job) = self.job.take() else {
            return Ok(None);
        };
        let started = std::time::Instant::now();
        for _ in 0..STEPS_PER_FRAME {
            if job.sim.tick >= job.target {
                break;
            }
            Self::apply_at(&mut job, self.tape.as_ref().ok_or("Запись потеряна")?)?;
            job.effects.before(&job.sim);
            job.sim.step();
            job.effects.capture(&job.sim);
            if job.sim.tick < job.target && job.sim.tick % CACHE_INTERVAL == 0 {
                self.cache_job(&job);
            }
            if started.elapsed() >= std::time::Duration::from_millis(4) {
                break;
            }
        }
        if job.sim.tick < job.target {
            self.job = Some(job);
            return Ok(None);
        }
        // Commands submitted on the terminal tick are part of the stopped state.
        let tape = self.tape.as_ref().ok_or("Запись потеряна")?;
        if job.target == tape.end {
            Self::apply_at(&mut job, tape)?;
            if job.sim.state_hash() != tape.end_hash {
                self.target = None;
                return Err("Состояние последнего тика не совпало с записью".into());
            }
        } else if job.target > tape.start {
            self.cache_job(&job);
        }
        self.position = Some(job.target);
        self.target = None;
        Ok(Some((job.sim, job.effects)))
    }
    pub fn cancel(&mut self) {
        self.job = None;
        self.target = None;
    }
    pub fn leave(&mut self) {
        self.cancel();
        self.playing = false;
        self.position = None;
    }
    pub fn play(&mut self, sim: &BlastRange) -> Result<(), String> {
        if self.recording || self.job.is_some() {
            return Err("Сначала остановите запись и дождитесь перемотки".into());
        }
        let tape = self.tape.as_ref().ok_or("Нет записи")?;
        if self.position != Some(sim.tick) {
            return Err("Сначала выберите тик записи ползунком".into());
        }
        if sim.tick >= tape.end {
            return Err("Достигнут конец записи: выберите более ранний тик".into());
        }
        self.play_next = tape.records.partition_point(|r| r.assigned_tick < sim.tick);
        self.playing = true;
        Ok(())
    }
    pub fn before_tick(&mut self, sim: &mut BlastRange) -> Result<(), String> {
        let tape = self.tape.as_ref().ok_or("Запись потеряна")?;
        while let Some(r) = tape
            .records
            .get(self.play_next)
            .filter(|r| r.assigned_tick == sim.tick)
        {
            let outcome = sim.execute_command(r.command);
            if outcome != r.outcome {
                self.playing = false;
                return Err(format!("Команда на тике {} воспроизведена иначе", sim.tick));
            }
            self.play_next += 1;
        }
        Ok(())
    }
    pub fn after_tick(&mut self, sim: &mut BlastRange) -> Result<bool, String> {
        let end = self.tape.as_ref().ok_or("Запись потеряна")?.end;
        self.position = Some(sim.tick);
        if sim.tick < end {
            return Ok(false);
        }
        self.before_tick(sim)?;
        self.playing = false;
        if sim.state_hash() != self.tape.as_ref().unwrap().end_hash {
            return Err("Состояние последнего тика не совпало с записью".into());
        }
        Ok(true)
    }
    pub fn save(&mut self) -> Result<String, String> {
        self.save_in(std::path::Path::new("out/material-lab/recordings"))
    }
    fn save_in(&mut self, dir: &std::path::Path) -> Result<String, String> {
        if self.recording {
            return Err("Сначала остановите запись".into());
        }
        let tape = self.tape.as_ref().ok_or("Нет записи")?;
        let bytes = tape.encode();
        if bytes.len() > MAX_FILE {
            return Err("Файл записи превышает 192 МиБ".into());
        }
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_millis();
        for suffix in 0..1000 {
            let path = dir.join(format!("{now:020}-{suffix:03}.rtsr"));
            let temp = path.with_extension("tmp");
            if path.exists() {
                continue;
            }
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp)
            {
                Ok(mut f) => {
                    if let Err(e) = f.write_all(&bytes) {
                        drop(f);
                        let _ = std::fs::remove_file(&temp);
                        return Err(e.to_string());
                    }
                    drop(f);
                    if let Err(e) = std::fs::rename(&temp, &path) {
                        let _ = std::fs::remove_file(&temp);
                        return Err(e.to_string());
                    }
                    self.opened = Some(path.clone());
                    return Ok(path.display().to_string());
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.to_string()),
            }
        }
        Err("Не удалось выбрать имя файла записи".into())
    }
    pub fn open(&mut self, delta: i32) -> Result<String, String> {
        self.open_in(std::path::Path::new("out/material-lab/recordings"), delta)
    }
    fn open_in(&mut self, dir: &std::path::Path, delta: i32) -> Result<String, String> {
        if self.recording {
            return Err("Сначала остановите запись".into());
        }
        let mut paths: Vec<_> = std::fs::read_dir(dir)
            .map_err(|e| e.to_string())?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|v| v == "rtsr"))
            .collect();
        paths.sort();
        if paths.is_empty() {
            return Err("Сохранённые записи не найдены".into());
        }
        let current = self
            .opened
            .as_ref()
            .and_then(|p| paths.iter().position(|v| v == p))
            .unwrap_or(paths.len() - 1);
        let index = if delta == 0 {
            paths.len() - 1
        } else {
            (current as i64 + i64::from(delta)).clamp(0, (paths.len() - 1) as i64) as usize
        };
        let path = paths.swap_remove(index);
        if std::fs::metadata(&path).map_err(|e| e.to_string())?.len() > MAX_FILE as u64 {
            return Err("Файл записи слишком велик".into());
        }
        let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
        let tape = Tape::decode(&bytes).ok_or("Файл записи повреждён или несовместим")?;
        self.cache_enabled = tape.snapshot.len() <= CACHE_BYTES;
        self.tape = Some(tape);
        self.job = None;
        self.cache.clear();
        self.playing = false;
        self.position = None;
        self.target = None;
        self.opened = Some(path.clone());
        Ok(path.display().to_string())
    }
    pub fn text(&self, tick: u32) -> String {
        if self.recording {
            return format!(
                "● Запись · тик {} · остановите запись для просмотра",
                self.tape.as_ref().map_or(tick, |t| t.end)
            );
        }
        let Some((a, b)) = self.bounds() else {
            return "Запись отсутствует".into();
        };
        if let Some(target) = self.target {
            format!(
                "Перемотка: {} → {target} · {}…{}",
                self.job.as_ref().map_or(a, |j| j.sim.tick),
                a,
                b
            )
        } else {
            format!(
                "{}Тик {} / {} · начало {}",
                if self.playing { "▶ " } else { "" },
                self.position.unwrap_or(tick),
                b,
                a
            )
        }
    }
}

#[derive(Component)]
pub struct Track;
#[derive(Component)]
pub struct Fill;
fn tick_at(a: u32, b: u32, x: f32) -> u32 {
    a + ((b - a) as f64 * x.clamp(0.0, 1.0) as f64).round() as u32
}
pub fn track(p: &mut ChildSpawnerCommands, f: &TextFont) {
    p.spawn((
        Text::new("Позиция по тикам"),
        f.clone(),
        TextColor(theme::MUTED),
    ));
    p.spawn((
        Button,
        Track,
        bevy::ui::RelativeCursorPosition::default(),
        Node {
            width: percent(100),
            height: px(32),
            border: UiRect::all(px(1)),
            ..default()
        },
        BackgroundColor(theme::BUTTON),
        BorderColor::all(theme::FRAME),
    ))
    .with_children(|p| {
        p.spawn((
            Fill,
            Node {
                position_type: PositionType::Absolute,
                left: px(0),
                top: px(0),
                bottom: px(0),
                width: percent(0),
                ..default()
            },
            BackgroundColor(theme::ACCENT),
        ));
    });
}
pub fn input(
    mut lab: ResMut<Laboratory>,
    mouse: Res<ButtonInput<MouseButton>>,
    tracks: Query<(&Interaction, &bevy::ui::RelativeCursorPosition), With<Track>>,
    mut fills: Query<&mut Node, With<Fill>>,
    mut drag: Local<Option<u32>>,
) {
    if !lab.active() {
        *drag = None;
        return;
    }
    if let Some((a, b)) = lab.replay.bounds() {
        if mouse.pressed(MouseButton::Left) {
            for (interaction, cursor) in &tracks {
                if *interaction == Interaction::Pressed {
                    if let Some(p) = cursor.normalized {
                        // Bevy normalizes node coordinates around its center.
                        *drag = Some(tick_at(a, b, p.x + 0.5));
                    }
                }
            }
        }
        if mouse.just_released(MouseButton::Left) {
            if let Some(tick) = drag.take() {
                lab.constructor.pending.push(LabAction::ReplaySeek(tick));
            }
        }
        let at = drag
            .or(lab.replay.target)
            .or(lab.replay.position)
            .unwrap_or(a)
            .clamp(a, b);
        let pct = if b > a {
            100.0 * (at - a) as f32 / (b - a) as f32
        } else {
            0.0
        };
        for mut node in &mut fills {
            node.width = percent(pct);
        }
    } else {
        *drag = None;
        for mut node in &mut fills {
            node.width = percent(0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sim() -> BlastRange {
        let config =
            ExplosionConfig::from_ron(include_str!("../../../../../../data/explosion.ron"))
                .unwrap();
        let mut layout =
            RangeLayout::from_ron(include_str!("../../../../../../data/range-cover.ron")).unwrap();
        layout.blocks.clear();
        layout.units.clear();
        layout.shields.clear();
        layout.course = None;
        layout.laboratory = true;
        let mut sim = BlastRange::with_layout(config, layout).unwrap();
        sim.set_physics_workers(1);
        sim.set_navigation_workers(1);
        sim
    }
    #[test]
    fn slider_reaches_endpoints_and_every_tick() {
        assert_eq!(tick_at(10, 110, -1.0), 10);
        assert_eq!(tick_at(10, 110, 2.0), 110);
        for i in 0..=100 {
            assert_eq!(tick_at(10, 110, i as f32 / 100.0), 10 + i);
        }
    }
    fn finish(replay: &mut Replay) -> BlastRange {
        for _ in 0..100 {
            if let Some((s, _)) = replay.advance().unwrap() {
                return s;
            }
        }
        panic!("replay did not finish")
    }
    fn finish_with_effects(replay: &mut Replay) -> (BlastRange, fire::Effects) {
        for _ in 0..100 {
            if let Some(result) = replay.advance().unwrap() {
                return result;
            }
        }
        panic!("replay did not finish")
    }
    #[test]
    fn recording_restores_each_tick_with_commands_and_terminal_edit() {
        let mut sim = sim();
        let mut replay = Replay::default();
        replay.start(&mut sim).unwrap();
        let mut hashes = vec![sim.state_hash()];
        for tick in 0..8 {
            if tick == 1 || tick == 4 {
                assert!(sim
                    .execute_command(Command::Sculpt {
                        at: [100 + tick, 25, 100],
                        edge: 1,
                        material: 11,
                        erase: false
                    })
                    .accepted());
            }
            sim.step();
            hashes.push(sim.state_hash());
        }
        assert!(sim
            .execute_command(Command::Sculpt {
                at: [105, 25, 102],
                edge: 1,
                material: 10,
                erase: false
            })
            .accepted());
        let end = sim.state_hash();
        replay.stop(&mut sim).unwrap();
        for tick in [0, 1, 3, 5, 7] {
            replay.seek(tick).unwrap();
            let restored = finish(&mut replay);
            assert_eq!(restored.tick, tick);
            assert_eq!(restored.state_hash(), hashes[tick as usize]);
        }
        replay.seek(8).unwrap();
        assert_eq!(finish(&mut replay).state_hash(), end);
        replay.seek(2).unwrap();
        assert_eq!(finish(&mut replay).state_hash(), hashes[2]);
        let bytes = replay.tape.as_ref().unwrap().encode();
        let decoded = Tape::decode(&bytes).expect("portable recording");
        let mut loaded = Replay {
            tape: Some(decoded),
            ..Default::default()
        };
        loaded.seek(8).unwrap();
        assert_eq!(finish(&mut loaded).state_hash(), end);
        loaded.seek(3).unwrap();
        assert_eq!(finish(&mut loaded).state_hash(), hashes[3]);
        loaded.seek(2).unwrap();
        let mut played = finish(&mut loaded);
        loaded.play(&played).unwrap();
        while loaded.playing {
            loaded.before_tick(&mut played).unwrap();
            played.step();
            loaded.after_tick(&mut played).unwrap();
        }
        assert_eq!(played.state_hash(), end);
        let mut damaged = bytes.clone();
        damaged[20] ^= 1;
        assert!(Tape::decode(&damaged).is_none());
        assert!(Tape::decode(&bytes[..bytes.len() - 1]).is_none());
    }
    #[test]
    fn missing_and_running_recordings_cannot_seek() {
        let mut r = Replay::default();
        assert!(r.seek(0).is_err());
        let mut sim = sim();
        r.start(&mut sim).unwrap();
        assert!(r.seek(0).is_err());
        r.stop(&mut sim).unwrap();
        assert_eq!(r.bounds(), Some((0, 0)));
    }
    #[test]
    fn replay_preserves_weapon_changes_and_live_stream_commands() {
        use sim_core::blast::battle::commands::GunCommand;
        let mut sim = sim();
        sim.execute_command(Command::Gun(GunCommand::Control));
        let mut r = Replay::default();
        r.start(&mut sim).unwrap();
        let laser =
            ExplosionConfig::from_ron(include_str!("../../../../../../data/laser.ron")).unwrap();
        assert!(sim
            .execute_command(Command::ConfigureWeapon { config: laser })
            .accepted());
        assert!(sim
            .execute_command(Command::Gun(GunCommand::LabStream {
                origin: [100, 40, 100],
                target: [130, 40, 100],
                enabled: true
            }))
            .accepted());
        for _ in 0..5 {
            sim.step();
        }
        assert!(sim
            .execute_command(Command::Gun(GunCommand::LabStream {
                origin: [0; 3],
                target: [0; 3],
                enabled: false
            }))
            .accepted());
        for _ in 0..3 {
            sim.step();
        }
        let expected = sim.state_hash();
        r.stop(&mut sim).unwrap();
        let bytes = r.tape.as_ref().unwrap().encode();
        let mut loaded = Replay {
            tape: Some(Tape::decode(&bytes).unwrap()),
            cache_enabled: true,
            ..Default::default()
        };
        loaded.seek(5).unwrap();
        let _ = finish(&mut loaded);
        assert!(loaded.cache.iter().any(|c| c.tick == 5));
        loaded.seek(7).unwrap();
        assert_eq!(loaded.job.as_ref().unwrap().sim.tick, 5);
        let _ = finish(&mut loaded);
        loaded.seek(8).unwrap();
        assert_eq!(loaded.job.as_ref().unwrap().sim.tick, 7);
        assert_eq!(finish(&mut loaded).state_hash(), expected);
        loaded.seek(0).unwrap();
        let mut played = finish(&mut loaded);
        loaded.play(&played).unwrap();
        while loaded.playing {
            loaded.before_tick(&mut played).unwrap();
            played.step();
            loaded.after_tick(&mut played).unwrap();
        }
        assert_eq!(played.state_hash(), expected);
    }
    #[test]
    fn seek_retargets_pending_work_and_clears_cache_on_new_recording() {
        let mut sim = sim();
        let mut replay = Replay::default();
        replay.start(&mut sim).unwrap();
        for _ in 0..12 {
            sim.step();
        }
        replay.stop(&mut sim).unwrap();
        replay.seek(8).unwrap();
        let _ = replay.advance().unwrap();
        let progress = replay.job.as_ref().unwrap().sim.tick;
        replay.seek(10).unwrap();
        assert_eq!(replay.job.as_ref().unwrap().sim.tick, progress);
        let _ = finish(&mut replay);
        assert!(!replay.cache.is_empty());
        replay.start(&mut sim).unwrap();
        assert!(replay.cache.is_empty());
        replay.stop(&mut sim).unwrap();
    }
    #[test]
    fn underground_explosion_replays_physical_state_at_intermediate_ticks() {
        use sim_core::blast::battle::commands::GunCommand;
        let config =
            ExplosionConfig::from_ron(include_str!("../../../../../../data/explosion.ron"))
                .unwrap();
        let mut layout =
            RangeLayout::from_ron(include_str!("../../../../../../data/range-cover.ron")).unwrap();
        layout.size = [128, 128, 128];
        layout.clear_min = [1, 1];
        layout.clear_max = [127, 127];
        layout.gun = [40, 24, 64];
        layout.gun_point = true;
        layout.target = [64, 16, 64];
        layout.blocks.clear();
        layout.units.clear();
        layout.shields.clear();
        layout.course = None;
        layout.laboratory = true;
        let mut sim = BlastRange::with_layout(config, layout).unwrap();
        sim.set_physics_workers(1);
        sim.set_navigation_workers(1);
        sim.execute_command(Command::Gun(GunCommand::Control));
        let mut replay = Replay::default();
        replay.start(&mut sim).unwrap();
        assert!(sim
            .execute_command(Command::Gun(GunCommand::Detonate { at: [64, 8, 64] }))
            .accepted());
        let mut effects = fire::Effects::default();
        effects.before(&sim);
        let mut states = vec![sim.state_hash()];
        let mut visuals = vec![effects.replay_counts()];
        let mut particles = vec![effects.replay_particles()];
        for _ in 0..8 {
            effects.before(&sim);
            sim.step();
            effects.capture(&sim);
            states.push(sim.state_hash());
            visuals.push(effects.replay_counts());
            particles.push(effects.replay_particles());
        }
        replay.stop(&mut sim).unwrap();
        assert!(
            visuals.iter().any(|v| v.0 > 0),
            "the scenario should create transient particles"
        );
        for tick in [1, 3, 6, 8] {
            replay.seek(tick).unwrap();
            let (restored, fx) = finish_with_effects(&mut replay);
            assert_eq!(restored.state_hash(), states[tick as usize]);
            assert_eq!(fx.replay_counts(), visuals[tick as usize]);
            assert_eq!(fx.replay_particles(), particles[tick as usize]);
        }
        let bytes = replay.tape.as_ref().unwrap().encode();
        let mut reopened = Replay {
            tape: Some(Tape::decode(&bytes).unwrap()),
            ..Default::default()
        };
        reopened.seek(6).unwrap();
        let (_, fx) = finish_with_effects(&mut reopened);
        assert_eq!(fx.replay_particles(), particles[6]);
    }
    #[test]
    fn recording_can_begin_after_the_simulation_has_started() {
        let mut sim = sim();
        for _ in 0..3 {
            sim.step();
        }
        let initial = sim.state_hash();
        let mut r = Replay::default();
        r.start(&mut sim).unwrap();
        assert!(sim
            .execute_command(Command::Sculpt {
                at: [100, 25, 100],
                edge: 1,
                material: 11,
                erase: false
            })
            .accepted());
        for _ in 0..2 {
            sim.step();
        }
        let end = sim.state_hash();
        r.stop(&mut sim).unwrap();
        assert_eq!(r.bounds(), Some((3, 5)));
        r.seek(3).unwrap();
        assert_eq!(finish(&mut r).state_hash(), initial);
        r.seek(5).unwrap();
        assert_eq!(finish(&mut r).state_hash(), end);
    }
    #[test]
    fn saved_recordings_have_unique_names_and_can_be_browsed() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("rts-vox-replay-{}-{unique}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        let mut sim = sim();
        let mut r = Replay::default();
        r.start(&mut sim).unwrap();
        r.stop(&mut sim).unwrap();
        let first = r.save_in(&dir).unwrap();
        let second = r.save_in(&dir).unwrap();
        assert_ne!(first, second);
        let mut loaded = Replay::default();
        assert_eq!(loaded.open_in(&dir, 0).unwrap(), second);
        assert_eq!(loaded.open_in(&dir, -1).unwrap(), first);
        assert_eq!(loaded.open_in(&dir, 1).unwrap(), second);
        loaded.seek(0).unwrap();
        assert_eq!(finish(&mut loaded).state_hash(), sim.state_hash());
        std::fs::remove_file(first).unwrap();
        std::fs::remove_file(second).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }
}
