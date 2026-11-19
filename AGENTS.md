# Working agreement

Read CLAUDE.md for engine invariants. For the current UI work, read
docs/ui-optical-work.md before searching historical reports again.

- Follow-up messages steer the active task unless the user explicitly cancels
  it. Answer direct questions promptly, then resume the unfinished work.
- Preserve the user's queue mode. The reliability task is to prevent an active
  run from stalling and blocking that queue. Do not replace queueing with steer,
  reduce context limits, or treat either change as a fix for a stalled run.
- Keep a concrete done/remaining ledger. Do not end with only a promise to
  continue while authorized implementation remains. Do not claim background or
  overnight work unless an actual active run is verified.
- After an interruption, inspect the last command result and working tree;
  continue from there. Reuse already-read sources and existing evidence.
- A progress update describes an actual edit, finding, test or running process.
  Read-only inspection is not implementation. Avoid repeated broad scans.
- Use scripts/test-fast.py and optimized, GPU-free mechanics checks where
  applicable. Native UI changes use focused UI checks and a few native captures.
  Do not repeatedly run the expensive debug full-world fixtures.
- Start walking changes with `--suite movement-quick`; add `--suite supports`
  for support/terrain and `--suite walking-blasts` for actual explosions.
  Reuse only whitelisted pure passes; keep every explicit `--skip` in evidence.
- Iterate in the cached development build. Build release once after the visual
  iteration passes. Bound CPU workers and use background windows, respecting
  the existing 200 FPS ceiling. Paused screenshots are not battle FPS evidence.
- Keep diagnostic output focused: do not print full logs, base64 images or giant
  transcripts. This is not authorization to reduce model context. Use SQLite's
  paginated public-item history instead of rescanning the 5.9 GB rollout.
- Do not open full-resolution PNG captures or references for a general visual
  pass. Run `python scripts/image-preview.py <image...>` and inspect the
  companion `*.preview.jpg` files instead. Keep originals unchanged for targeted
  detail checks; use a crop when full-resolution detail is actually required.
- A network/queue failure is not completion. Record exact timestamps and errors,
  distinguish a saved setting from a verified live change, and preserve user
  queues. Never bypass the app's safety rules, mutate its live databases, or
  install an automatic message/restart loop as an unverified workaround.
