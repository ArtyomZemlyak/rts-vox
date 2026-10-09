//! Qualification of the command owner against the same direct battle kernel.
use sim_core::blast::{
    ai::Rules,
    battle::{
        commands::{
            runtime::{
                AiFrame, Checkpoint, CommandId, CommandRuntime, DecodeLimits, Envelope, Limits,
                PlanEnvelope, PlanState,
            },
            Command,
        },
        Order,
    },
    BlastRange,
};
use std::{io::Write, time::Instant};

enum Run {
    Direct(Box<BlastRange>),
    Owned(Box<CommandRuntime>),
}
impl Run {
    fn world(&self) -> &BlastRange {
        match self {
            Self::Direct(world) => world,
            Self::Owned(owner) => owner.world(),
        }
    }
    fn step(&mut self) {
        match self {
            Self::Direct(world) => world.step(),
            Self::Owned(owner) => owner.step().unwrap(),
        }
    }
    fn record_ai(&mut self) -> Option<AiFrame> {
        let Self::Owned(owner) = self else {
            unreachable!()
        };
        owner.step_record_ai().unwrap()
    }
    fn plan(&mut self, envelope: PlanEnvelope) -> CommandId {
        match self {
            Self::Direct(world) => {
                let id = CommandId {
                    epoch: envelope.epoch,
                    ordinal: envelope.sequence,
                };
                world
                    .submit_enemy_ai_plan(envelope.decision_tick, envelope.decisions)
                    .unwrap();
                id
            }
            Self::Owned(owner) => owner.submit_plan(envelope).unwrap(),
        }
    }
}
fn main() {
    let args: Vec<_> = std::env::args().collect();
    let value = |name: &str| {
        args.windows(2)
            .find(|w| w[0] == name)
            .map(|w| w[1].as_str())
    };
    let ticks: u32 = value("--ticks").unwrap_or("3600").parse().unwrap();
    let pending: usize = value("--pending").unwrap_or("0").parse().unwrap();
    let owned = args.iter().any(|arg| arg == "--owned");
    let plans = args.iter().any(|arg| arg == "--plans");
    let portable = args.iter().any(|arg| arg == "--portable-replay");
    let ai_replay = args.iter().any(|arg| arg == "--ai-replay");
    let replay_from = value("--replay-from").map(|value| value.parse::<u32>().unwrap());
    assert!(ticks > 0 && ticks < u32::MAX && (owned || pending == 0));
    assert!(
        !plans || pending == 0,
        "--plans and --pending are separate workloads"
    );
    assert!(replay_from.is_none_or(|tick| owned && tick > 0 && tick < ticks));
    assert!(!portable || replay_from.is_some());
    assert!(
        !ai_replay || (owned && replay_from.is_some()) || value("--verify-checkpoint").is_some()
    );
    assert_eq!(ai_replay, value("--ai-frame-dir").is_some());
    assert!(
        value("--checkpoint-out").is_none() || portable || value("--verify-checkpoint").is_some()
    );
    let mut layout =
        modding::range::RangeLayout::from_ron(include_str!("../../../../data/range-km.ron"))
            .unwrap();
    let preset = modding::range_skirmish::WorldBattle::from_ron(include_str!(
        "../../../../data/battle-world.ron"
    ))
    .unwrap();
    layout.battle = true;
    if let Some(course) = &mut layout.course {
        course.patrols.clear();
    }
    layout.units = preset.units;
    layout.blocks.extend(preset.blocks);
    let config = modding::explosion::ExplosionConfig::from_ron(include_str!(
        "../../../../data/explosion.ron"
    ))
    .unwrap();
    let decode_limits = DecodeLimits {
        world_bytes: 256 * 1024 * 1024,
        runtime: Limits {
            retained: pending.max(1024),
            producers: 1,
            per_tick: 64,
        },
        plan_decisions: layout.units.len(),
    };
    if let Some(path) = value("--verify-checkpoint") {
        let bytes = std::fs::read(path).unwrap();
        let checkpoint = Checkpoint::from_bytes(&bytes, decode_limits).unwrap();
        assert_eq!(bytes, checkpoint.to_bytes());
        let owner = CommandRuntime::restore(config, checkpoint).unwrap();
        let canonical = owner.checkpoint().to_bytes();
        assert_eq!(bytes, canonical);
        if let Some(path) = value("--checkpoint-out") {
            std::fs::write(path, canonical).unwrap();
        }
        println!(
            "portable_verify tick={} retained={} owner_hash={:016x} physical_hash={:016x}",
            owner.world().tick,
            owner.retained(),
            owner.state_hash(),
            owner.world().state_hash()
        );
        if ai_replay {
            let mut owner = owner;
            let from = owner.world().tick;
            assert!(from < ticks);
            let mut frames = 0;
            while owner.world().tick < ticks {
                let path = std::path::Path::new(value("--ai-frame-dir").unwrap())
                    .join(format!("{:010}.ai", owner.world().tick));
                let frame = if path.exists() {
                    assert!(
                        std::fs::metadata(&path).unwrap().len()
                            <= 49 + 17 * layout.units.len() as u64
                    );
                    frames += 1;
                    Some(
                        AiFrame::from_bytes(&std::fs::read(path).unwrap(), layout.units.len())
                            .unwrap(),
                    )
                } else {
                    None
                };
                owner.step_replay_ai(frame).unwrap();
            }
            if let Some(path) = value("--snapshot-out") {
                std::fs::write(path, owner.world().snapshot()).unwrap();
            }
            if let Some(path) = value("--final-checkpoint-out") {
                std::fs::write(path, owner.checkpoint().to_bytes()).unwrap();
            }
            println!("ai_replay from={from} to={ticks} frames={frames} owner_hash={:016x} physical_hash={:016x}", owner.state_hash(), owner.world().state_hash());
        }
        return;
    }
    let mut world = BlastRange::with_layout(config, layout).unwrap();
    assert!(world.set_ai_vs_ai(true));
    assert!(world.set_navigation_workers(8));
    world.start_battle().unwrap();
    let mut run = if owned {
        // Diagnostic owner settings, not a new default gameplay quota.
        let mut owner = CommandRuntime::new(
            world,
            1,
            Limits {
                retained: pending.max(1024),
                producers: 1,
                per_tick: 64,
            },
        )
        .unwrap();
        for sequence in 0..pending {
            owner
                .submit(Envelope {
                    epoch: 1,
                    producer: 0,
                    sequence: sequence as u64,
                    requested_tick: ticks,
                    command: Command::Order {
                        actor: usize::MAX,
                        order: Order::Hold,
                        destination: None,
                    },
                })
                .unwrap();
        }
        Run::Owned(Box::new(owner))
    } else {
        Run::Direct(Box::new(world))
    };
    let mut rows = Vec::with_capacity(ticks as usize);
    let mut windows = Vec::new();
    let mut peak_rounds = 0;
    let mut replica: Option<CommandRuntime> = None;
    let mut plan_ids = Vec::new();
    let mut inputs = Vec::new();
    let mut plan_pending = false;
    let mut delivery_ns = 0;
    let mut frame_count = 0;
    let mut frame_bytes = 0;
    if let Some(path) = value("--ai-frame-dir") {
        std::fs::create_dir_all(path).unwrap();
    }
    for _ in 0..ticks {
        if plans && run.world().battle.phase == sim_core::blast::battle::Phase::Fighting {
            let observation = run.world().enemy_ai_observation().unwrap();
            // Deliver one tick before the deadline when possible, exercising
            // pending state without shifting the existing AI decision phase.
            if !plan_pending
                && observation.decision_tick < ticks
                && observation.decision_tick <= run.world().tick.saturating_add(1)
            {
                let decisions = Rules {
                    tuning: run.world().enemy_ai_tuning(),
                    radii: run.world().enemy_ai_radii(),
                    controlled_allied: false,
                }
                .whole_plan(&observation);
                let envelope = PlanEnvelope {
                    epoch: 1,
                    producer: 0,
                    sequence: plan_ids.len() as u64,
                    decision_tick: observation.decision_tick,
                    decisions,
                };
                if let Some(replica) = &mut replica {
                    replica.submit_plan(envelope.clone()).unwrap();
                }
                inputs.push((run.world().tick, envelope.clone()));
                let start = Instant::now();
                plan_ids.push(run.plan(envelope));
                delivery_ns += start.elapsed().as_nanos();
                plan_pending = true;
            }
        }
        let start = Instant::now();
        let frame = if ai_replay && replica.is_some() {
            run.record_ai()
        } else {
            run.step();
            None
        };
        let ns = start.elapsed().as_nanos();
        if plans {
            plan_pending = match &run {
                Run::Direct(world) => {
                    world.battle.phase == sim_core::blast::battle::Phase::Fighting
                        && inputs
                            .last()
                            .is_some_and(|(_, input)| input.decision_tick >= world.tick)
                }
                Run::Owned(owner) => plan_ids.last().is_some_and(|&id| {
                    owner.plan_receipt(id).unwrap().state == PlanState::Delivered
                }),
            };
        }
        let world = run.world();
        peak_rounds = peak_rounds.max(world.battle.rounds.len());
        rows.push((
            world.tick,
            ns,
            world.battle.rounds.len(),
            world.matter.debris.fragments.len(),
            world.matter.felling_pending(),
            world.collapse_work.queued,
            world
                .battle
                .actors
                .iter()
                .map(|actor| actor.fired as u64)
                .sum::<u64>(),
        ));
        if world.tick % 300 == 0 {
            windows.push((world.tick, world.state_hash()));
        }
        if let Some(replica) = &mut replica {
            if ai_replay {
                let frame = frame.map(|frame| {
                    let bytes = frame.to_bytes();
                    frame_count += 1;
                    frame_bytes += bytes.len();
                    let decoded = AiFrame::from_bytes(&bytes, world.battle.actors.len()).unwrap();
                    assert_eq!(frame, decoded);
                    std::fs::write(
                        std::path::Path::new(value("--ai-frame-dir").unwrap())
                            .join(format!("{:010}.ai", frame.tick())),
                        bytes,
                    )
                    .unwrap();
                    decoded
                });
                replica.step_replay_ai(frame).unwrap();
            } else {
                replica.step().unwrap();
            }
        }
        if replay_from == Some(world.tick) {
            let Run::Owned(owner) = &run else {
                unreachable!()
            };
            let checkpoint = owner.checkpoint();
            let checkpoint = if portable {
                let start = Instant::now();
                let bytes = checkpoint.to_bytes();
                let encode_ns = start.elapsed().as_nanos();
                let start = Instant::now();
                let decoded = Checkpoint::from_bytes(&bytes, decode_limits).unwrap();
                let decode_ns = start.elapsed().as_nanos();
                assert_eq!(bytes, decoded.to_bytes());
                if let Some(path) = value("--checkpoint-out") {
                    std::fs::write(path, &bytes).unwrap();
                }
                println!("portable_checkpoint tick={} retained={} bytes={} capacity={} encode_ns={encode_ns} decode_ns={decode_ns} owner_hash={:016x} physical_hash={:016x}", world.tick, owner.retained(), bytes.len(), bytes.capacity(), owner.state_hash(), world.state_hash());
                decoded
            } else {
                checkpoint
            };
            let restored = CommandRuntime::restore(world.config, checkpoint).unwrap();
            assert_eq!(owner.state_hash(), restored.state_hash());
            replica = Some(restored);
        }
    }
    if let Some(path) = value("--csv") {
        let mut out = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
        writeln!(
            out,
            "tick,total_ns,rounds,fragments,felling,collapse_queued,fired"
        )
        .unwrap();
        for &(tick, ns, rounds, fragments, felling, collapse, fired) in &rows {
            writeln!(
                out,
                "{tick},{ns},{rounds},{fragments},{felling},{collapse},{fired}"
            )
            .unwrap();
        }
    }
    let world = run.world();
    assert!(world.matter.economy.ledger.balanced());
    if plans {
        println!(
            "plan_delivery plans={} decisions={} delivery_ns={delivery_ns}",
            plan_ids.len(),
            inputs
                .iter()
                .map(|(_, input)| input.decisions.len())
                .sum::<usize>()
        );
        for (tick, input) in &inputs {
            println!(
                "plan_input delivered={tick} tick={} sequence={} decisions={:?}",
                input.decision_tick, input.sequence, input.decisions
            );
        }
        if let Run::Owned(owner) = &run {
            let mut attempts = 0;
            let mut fallbacks = 0;
            let mut not_selected = 0;
            let mut cancelled = 0;
            for &id in &plan_ids {
                let receipt = owner.plan_receipt(id).unwrap();
                if let PlanState::Executed {
                    attempts: results, ..
                } = &receipt.state
                {
                    attempts += results.len();
                    fallbacks += results.iter().filter(|result| result.fallback).count();
                    not_selected += receipt.envelope.decisions.len()
                        - results.iter().filter(|result| !result.fallback).count();
                } else if matches!(receipt.state, PlanState::Cancelled { .. }) {
                    cancelled += 1;
                } else {
                    panic!("unfinished plan at benchmark end: {:?}", receipt.state);
                }
            }
            println!("plan_results retained={} attempts={attempts} fallbacks={fallbacks} not_selected={not_selected} cancelled={cancelled}", owner.retained());
        }
    }
    if let Some(replica) = replica {
        let Run::Owned(owner) = &run else {
            unreachable!()
        };
        assert_eq!(owner.state_hash(), replica.state_hash());
        assert_eq!(world.snapshot(), replica.world().snapshot());
        println!(
            "checkpoint_replay from={} to={ticks} retained={} owner_hash={:016x}",
            replay_from.unwrap(),
            owner.retained(),
            owner.state_hash()
        );
    }
    if let Some(path) = value("--snapshot-out") {
        std::fs::write(path, world.snapshot()).unwrap();
    }
    if let Run::Owned(owner) = &run {
        if let Some(path) = value("--final-checkpoint-out") {
            std::fs::write(path, owner.checkpoint().to_bytes()).unwrap();
        }
        if ai_replay {
            println!("ai_replay from={} to={ticks} frames={frame_count} bytes={frame_bytes} owner_hash={:016x} physical_hash={:016x}", replay_from.unwrap(), owner.state_hash(), world.state_hash());
        }
    }
    let total: u128 = rows.iter().map(|row| row.1).sum();
    let mut sorted: Vec<_> = rows.iter().map(|row| row.1).collect();
    sorted.sort_unstable();
    println!("owned={owned} pending={pending} ticks={ticks} mean_ns={} p99_ns={} max_ns={} hash={:016x} peak_rounds={peak_rounds} alive={:?} bases={:?}", total / u128::from(ticks), sorted[(sorted.len() - 1) * 99 / 100], sorted.last().unwrap(), world.state_hash(), world.battle_counts(), world.battle_base_integrity());
    for (tick, hash) in windows {
        println!("window tick={tick} hash={hash:016x}");
    }
}
