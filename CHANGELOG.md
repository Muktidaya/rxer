# Changelog

## 0.2.0 — unreleased

rxer gains configurable stations and resilient internet-radio reception while
keeping decoding and network work off the audio callback.

### Added

- TOML station configuration: required `url`, optional `name` and
  `metadata_encoding`. Embedded station data includes KUSC; user station tables
  override matching defaults. `rxer kusc` and `rxer --list` use the merged config.
- Conventional platform config paths, `RXER_CONFIG`, and `--config PATH`.
- Bounded PLS/M3U resolution with relative URLs, nested playlists, and alternate
  entry failover on connection or decoder failure.
- ICY track metadata, legacy `ICY 200 OK` responses, UTF-8/Windows-1252 text
  handling, and configurable legacy metadata encodings.
- Automatic reconnect with bounded backoff, idle timeouts, and rebuffering.
  Audio callbacks return silence during starvation instead of waiting for data.
- Conventional HLS live/VOD reception: master playlists and audio renditions,
  packed AAC/MP3, supported MPEG-TS audio, fragmented MP4, initialization maps,
  byte ranges, and identity AES-128 encryption. Failed segment downloads can
  resume without replaying preceding completed segments.
- Worker-side sample-rate/channel conversion across stream changes.
- `--dev` and `--release` select existing local builds. `RXER_BUILD_DIR` can
  connect an installed executable to a developer's build directory. These flags
  never compile; ordinary invocation retains the installed PATH behavior.

### Changed

- **Default volume is now 100%, up from 50%.** This is unity amplitude gain
  (1.0, or 0 dB), with no loudness normalization or system-volume adjustment.
  Use `--volume 50` to retain the previous attenuation of approximately -6.02 dB.
- **Ctrl-C is the only quit key.** `q` and Escape no longer stop TUI playback.
- CLI events and errors use local `HH:MM:SS` timestamps. Playback rows show
  status, artist, and title with four spaces between fields. Artist/title
  splitting is best-effort; metadata is not guaranteed to be structured.
- The TUI shows reception status, track metadata, and elapsed session time.
- Station aliases beginning with `-` are rejected because they conflict with
  command-line options.

### Validation and limits

- Tests cover config precedence, playlist failover, ICY, codec fixtures,
  HLS progression and recovery, PCM starvation, cancellation, format changes,
  build dispatch, quit keys, and CLI formatting, with and without the TUI feature.
- CI checks Linux, macOS, Windows, and the Rust 1.88 minimum supported version.
  Native playback acceptance is currently on macOS: approximately one hour of
  uninterrupted KUSC listening with live metadata was user-confirmed. A separate
  muted native-output check verified recovery after a forced local disconnect.
- No local-file playback, visualizers, EQ, loudness normalization, or limiter.
- DRM/SAMPLE-AES, low-latency/delta HLS, adaptive bitrate/language controls,
  unsupported codecs (including AC-3 and LATM AAC), timed HLS metadata, and
  sample-accurate continuity across HLS segments remain outside this release.
- Unity volume does not guarantee clipping safety or bit-perfect output.
  Detailed resource bounds and protocol policies are in the [README](README.md).

This entry is prepared release material. The package remains at 0.1.0 until a
separate versioning and publication decision; no 0.2.0 release has been created.
