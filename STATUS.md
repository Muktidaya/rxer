# rxer development status

Unreleased work toward v0.2.0. The package version and v0.1.0 release are unchanged.

Implemented: embedded TOML station defaults with user overrides; platform config
paths, `--config` and `RXER_CONFIG`; PLS/M3U resolution; ICY title extraction;
idle-read timeouts, bounded retries and PCM rebuffering. README documents the
merge rules, resource bounds, retry policy, and unsupported cases.

Validation: formatting, strict Clippy, and deterministic tests in both feature
configurations; Rust 1.88 compatibility check. Tests exercise config precedence,
playlist and ICY parsing, HTTP errors, idle reads, EOF/stall recovery, format
changes, callback frame alignment, and receiver cancellation. A live KUSC check decoded 45,056 samples at 22,050 Hz, stereo. Native device and
interactive TUI acceptance remain manual checks. Remote CI owns cross-platform
execution evidence; local tests alone do not establish it.

Next release gate: review platform CI and perform an interactive playback/TUI
session. Version bump, crates.io publication, tags, and release creation require
a separate release decision.
