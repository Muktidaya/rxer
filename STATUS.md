# rxer development status

## v0.2.0 release checkpoint — 2026-09-12

Version 0.2.0 is implemented and audited, with [release notes](CHANGELOG.md).
The user authorized versioning, crates.io publication, and the GitHub tag/release
on 2026-09-12. The registry and GitHub release records own live publication status.

The [README](README.md) owns configuration, protocol policies, resource bounds,
controls, gain staging, and known limitations. Built-in station definitions are
embedded data. Network I/O, decoding, metadata parsing, and stream format
conversion stay off the audio callback; bounded PCM staging and nonblocking
queue reads are retained. The CLI uses local-clock timestamps and four-space
field separators. Default volume is unity gain; Ctrl-C is the sole quit key.

## Audit and validation

- All eight CI runs through `139bbf9` and the final preparation commit
  `32ca7d9` passed the Linux/macOS/Windows and Rust 1.88 CI matrix.
  The versioned release commit is checked by the same matrix before publication.
- Complete local formatting, strict Clippy, and test suites passed with default
  features and `--no-default-features`; Rust 1.88 checks passed in both modes.
- Inspection found an accepted but unusable alias form: names beginning with
  `-` collided with CLI options. Validation now rejects it, with regression
  coverage and documentation. The user's final help wording is included.
- Unit/integration fixtures exercise EOF and idle-stall reconnect, bounded PCM
  rebuffering, cancellation, config precedence, alternate-entry failover, ICY
  framing/encodings, HLS containers/encryption/ranges/live sequence advancement,
  failed-segment recovery, and sample-rate/channel changes.
- Fresh release-executable acceptance used a loopback WAV server that cut its
  first response short. rxer reported reconnecting, opened a second connection,
  resumed playing through muted native output, and exited successfully on SIGINT.
- Both local build selectors and explicit station overrides were exercised from
  outside the checkout. `--config` beat a deliberately invalid `RXER_CONFIG`;
  `--resolve` and `--list` returned the expected effective station definitions.
- `cargo package --locked --allow-dirty` packaged 32 files and successfully
  compiled the extracted crate. Embedded defaults, codec fixtures, and release
  notes are included; this created no published release.
- The user confirmed roughly one hour of uninterrupted KUSC listening with
  successive metadata updates on macOS. The TUI was visibly confirmed, and a
  muted terminal check verified q/Escape do not quit while Ctrl-C exits cleanly.
  This is not native listening acceptance for Linux or Windows.

## Remaining boundaries

No local-file playback, visualizers, EQ, normalization, or limiter were added.
DRM/SAMPLE-AES, low-latency/delta HLS, adaptive bitrate/language controls,
unsupported codecs, timed HLS metadata, and sample-accurate segment continuity
remain unsupported. Dependency adapters retain their tested pins. A source
review and synthetic tests do not establish compatibility with every broadcaster.

Future feature work follows a separate scope decision. No EQ, visualizer, or
additional playback controls are part of this release.
