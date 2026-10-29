# Laboratory experiment recording

The laboratory records an initial `BlastRange` snapshot and the bounded
external command trace. The simulation remains authoritative; the player can
stop recording, drag a timeline, move by one or ten ticks, and replay forwards.
Scrubbing restores the initial snapshot and recomputes the requested ticks in
small per-frame batches. It does not retain a full world snapshot for every
tick. During seeking it keeps up to three transient checkpoints, at most 48 MiB
in total, at viewed positions and at 240-tick intervals. A later seek restores
the closest preceding checkpoint; large snapshots disable the cache. Retargeting
can reuse a pending seek. Checkpoints are not written to the recording file.
The normal CPU effects capture also runs during seeking, and its bounded state
is included in transient checkpoints. Particles, tracers, flashes and nuclear
effects created after recording starts are rebuilt even after reopening a file.
The existing live renderer retains its mesh handles. Snapshot creation and a single slow simulation tick can exceed the per-frame
time target.

Recordings can be saved under `out/material-lab/recordings` and reopened using
the latest/previous/next controls. Each save gets a unique name. The file stores
the existing command encoding, initial weapon configuration, snapshot, tick
range, results of admitted commands and a checksum. It rejects oversized,
truncated, corrupt and unsupported input. On reaching the end of the tape,
the replay checks the physical state hash against the recorded one and reports
a divergence instead of silently claiming success. Scene rebuild stops an
active recording; viewing a tape is read-only until the user explicitly leaves
replay mode and continues from that state.

Limits are 128 MiB for the initial snapshot and 16,384 captured commands;
overflow invalidates the tape explicitly. Scrubbing reconstructs an approximate
GPU plume for the latest non-kinetic laboratory impact after the recording
started. It uses the current weapon configuration and terrain rather than the
historical evolving terrain; prior impacts and continuous-fire gas are not
reconstructed. The GPU field starts empty and catches up one substep per frame,
then returns to the usual 16-step budget. Effects created before recording
starts are not captured. Playback from that point produces new presentation effects as simulation proceeds. Camera
movement and editor interface state are not recorded. Playback assumes the
current engine and data remain compatible; its final state check detects a
divergence, but intermediate ticks are not independently hashed.

Focused tests cover arbitrary tick seeks, terminal-tick commands, recording
after a non-zero starting tick, water edits, weapon configuration and live
laser commands, forward playback, file corruption, unique save names and
browsing, underground detonation including particle positions, timeline input and cancellation. The
pre-existing checkpoint wire conformance vector remains valid.
