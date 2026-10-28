//! Best-effort runtime status. A slow terminal must never hold up a game tick.
//! Profile CSVs and authoritative events are recorded separately.
use std::io::{self, Write};
use std::sync::mpsc::{sync_channel, SyncSender};

const CAPACITY: usize = 8;

#[derive(Clone, Copy, Debug)]
pub enum Status {
    Progress {
        frame: u32,
        age: f32,
        removed: usize,
        pending: usize,
        bodies: usize,
        tick_us: u128,
    },
    Impact {
        tick: u32,
        at: [i32; 3],
    },
}

struct Message {
    status: Status,
    skipped: u64,
}

pub struct Log {
    sender: Option<SyncSender<Message>>,
    skipped: u64,
    dropped: u64,
}

impl Default for Log {
    fn default() -> Self {
        Self::with_writer(io::stdout()).unwrap_or_else(|error| {
            // Startup only. Failure to create an optional observer must not
            // switch the tick back to synchronous stdout.
            eprintln!("runtime status disabled: {error}");
            Self {
                sender: None,
                skipped: 0,
                dropped: 0,
            }
        })
    }
}

impl Log {
    fn with_writer(writer: impl Write + Send + 'static) -> io::Result<Self> {
        let (sender, receiver) = sync_channel::<Message>(CAPACITY);
        // No join on Drop: a blocked output pipe must not block shutdown.
        // The queue drains on disconnect if the sink is healthy; final
        // console lines are best effort when the process exits.
        std::thread::Builder::new()
            .name("runtime-status".into())
            .spawn(move || {
                let mut writer = writer;
                while let Ok(message) = receiver.recv() {
                    if write_message(&mut writer, message).is_err() {
                        break;
                    }
                }
            })?;
        Ok(Self {
            sender: Some(sender),
            skipped: 0,
            dropped: 0,
        })
    }

    pub fn emit(&mut self, status: Status) {
        let message = Message {
            status,
            skipped: self.skipped,
        };
        // Fixed-size records only: no formatting, I/O, waiting or unbounded
        // allocation on the render thread, even if the terminal stops reading.
        if self
            .sender
            .as_ref()
            .is_some_and(|sender| sender.try_send(message).is_ok())
        {
            self.skipped = 0;
        } else {
            self.skipped = self.skipped.saturating_add(1);
            self.dropped = self.dropped.saturating_add(1);
        }
    }

    pub fn dropped(&self) -> u64 {
        self.dropped
    }
}

fn write_message(writer: &mut impl Write, message: Message) -> io::Result<()> {
    if message.skipped > 0 {
        writeln!(writer, "RUNTIME_STATUS skipped={}", message.skipped)?;
    }
    match message.status {
        Status::Progress {
            frame,
            age,
            removed,
            pending,
            bodies,
            tick_us,
        } => writeln!(
            writer,
            "frame={frame} blast_age={age:.2}s removed={removed} pending_edits={pending} bodies={bodies} tick_ms={:.1}",
            tick_us as f64 / 1000.0,
        ),
        Status::Impact { tick, at } => writeln!(writer, "IMPACT tick={tick} at={at:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;

    fn status(tick: u32) -> Status {
        Status::Impact {
            tick,
            at: [1, 2, 3],
        }
    }

    #[test]
    fn full_queue_keeps_order_and_reports_loss_on_next_admitted_record() {
        let (sender, receiver) = sync_channel(CAPACITY);
        let mut log = Log {
            sender: Some(sender),
            skipped: 0,
            dropped: 0,
        };
        for tick in 0..CAPACITY as u32 + 3 {
            log.emit(status(tick));
        }
        assert_eq!(log.dropped(), 3);
        for tick in 0..CAPACITY as u32 {
            let message = receiver.try_recv().unwrap();
            assert!(matches!(message.status, Status::Impact { tick: t, .. } if t == tick));
            assert_eq!(message.skipped, 0);
        }
        log.emit(status(99));
        let message = receiver.try_recv().unwrap();
        assert_eq!(message.skipped, 3);
        assert_eq!(log.skipped, 0);
        assert_eq!(log.dropped(), 3);
    }

    #[test]
    fn blocked_sink_does_not_block_producer_or_drop() {
        struct Blocked {
            entered: std::sync::mpsc::Sender<()>,
            release: std::sync::mpsc::Receiver<()>,
        }
        impl Write for Blocked {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                self.entered.send(()).unwrap();
                self.release.recv().unwrap();
                Err(io::Error::other(format!(
                    "closed after {} bytes",
                    bytes.len()
                )))
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let (entered, entered_rx) = channel();
        let (release, release_rx) = channel();
        let mut log = Log::with_writer(Blocked {
            entered,
            release: release_rx,
        })
        .unwrap();
        log.emit(status(0));
        entered_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        for tick in 1..=CAPACITY as u32 + 1 {
            log.emit(status(tick));
        }
        assert_eq!(log.dropped(), 1);
        drop(log);
        // The writer remains blocked until after both emit and Drop returned.
        release.send(()).unwrap();
    }

    #[test]
    fn disconnected_or_disabled_sink_only_counts_loss() {
        let (sender, receiver) = sync_channel(CAPACITY);
        drop(receiver);
        let mut log = Log {
            sender: Some(sender),
            skipped: 0,
            dropped: 0,
        };
        log.emit(status(1));
        log.sender = None;
        log.emit(status(2));
        assert_eq!(log.dropped(), 2);
    }

    #[test]
    fn preserves_status_fields_and_explicit_loss_marker() {
        let mut output = Vec::new();
        write_message(
            &mut output,
            Message {
                skipped: 2,
                status: Status::Progress {
                    frame: 30,
                    age: 1.25,
                    removed: 7,
                    pending: 8,
                    bodies: 9,
                    tick_us: 1250,
                },
            },
        )
        .unwrap();
        assert_eq!(String::from_utf8(output).unwrap(),
            "RUNTIME_STATUS skipped=2\nframe=30 blast_age=1.25s removed=7 pending_edits=8 bodies=9 tick_ms=1.2\n");
    }
}
