# rxer development status

Unreleased work toward v0.2.0. Package version and v0.1.0 release are unchanged.

Implemented: embedded TOML stations and user overrides; platform config paths,
`--config` and `RXER_CONFIG`; PLS/M3U failover; ICY framing, legacy responses and
metadata encodings; idle timeouts, bounded reconnects and PCM rebuffering;
worker-side format normalization; conventional HLS live/VOD, master/audio
rendition selection, packed audio, MPEG-TS, fMP4, byte ranges, maps, and AES-128.
HLS reconnects retain the last active media sequence checkpoint.

The README owns merge rules, selection/retry policies, resource limits, and
unsupported protocol/codec cases. No local-file or visualizer feature was added.
ureq and rodio are pinned at their adapter-tested versions. Tests include small
synthetic codec fixtures with generation provenance.

Validation covers formatting, strict Clippy and tests with both feature
configurations, Rust 1.88 compatibility, codec containers, complete fMP4 VOD,
live sequence advancement, failed-segment recovery without replay, mixed audio
formats, ICY encodings, and HTTP/decoder failover. The network integration suite
also passed ten consecutive runs after correcting fixture-server socket mode.
Remote CI owns cross-platform execution evidence. Interactive device/TUI
acceptance remains manual.

Remaining scope: DRM/SAMPLE-AES, low-latency/delta HLS, adaptive bitrate/language
controls, unsupported codecs, timed HLS metadata, and sample-accurate segment
continuity. None is silently claimed as supported.

Next release gate: review platform CI and perform interactive playback/TUI
acceptance. Version bump, publication, tags, and release creation require a
separate release decision.

Local build selection: `--dev` and `--release` dispatch to existing sibling
builds or an explicit `RXER_BUILD_DIR`; ordinary invocation retains PATH behavior.
Validation: build selection unit/process tests, both feature suites, strict
Clippy, formatting and Rust 1.88 checks pass locally. Both compiled profiles
were invoked successfully from outside the checkout.

Ctrl-C is the only quit key in CLI and TUI modes; q and Escape are ignored.
Initial volume defaults to 100; explicit --volume values still override it.
Validation: both feature suites and strict Clippy pass; rebuilt both profiles.
Muted terminal playback survived q/Escape and exited cleanly on Ctrl-C.

Gain contract: default 100% is unity amplitude (0 dB gain), with attenuation
only below 100. README distinguishes gain from loudness/full scale and records
explicit headroom requirements for future DSP. No normalization or limiter added.

CLI events use local-clock time/status/artist/title rows on stderr; metadata
splitting is best-effort and command-result stdout remains unchanged.

CLI events use space-padded columns without quotes or borders; control
characters in metadata become spaces and empty trailing fields are omitted.
The connecting event has no quit-key reminder.
