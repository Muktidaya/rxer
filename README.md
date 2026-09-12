# rxer

A lean terminal audio receiver and router.

`rxer` receives HTTP(S) internet radio, decodes it in Rust, and plays it through
the default audio output. An optional Ratatui display shows reception status and
track titles. The configuration and recovery features below are unreleased work
toward 0.2.0; the published 0.1.0 release has the original direct-stream behavior.

```sh
rxer kusc
rxer --tui kusc
rxer --volume 25 https://example.org/radio.mp3
rxer --check kusc
rxer --resolve kusc
```

## Install

Requires Rust 1.88 or newer and a working system audio output.

```sh
cargo install rxer --locked
```

To build from this repository:

```sh
cargo install --path . --locked
```

On Debian/Ubuntu, building also requires `pkg-config` and `libasound2-dev`.
macOS uses CoreAudio; Windows uses the system audio backend. There is no FFmpeg,
VLC, or external player dependency. TLS and audio device access still rely on
platform facilities through Rust ecosystem libraries.

Use `--no-default-features` with Cargo to omit Ratatui and its terminal backend.
Plain CLI playback remains available in that build.

## Controls and scope

- `--list`: list effective station aliases, names, and URLs.
- `--config PATH`: choose a TOML configuration file.
- `--resolve`: print the configured URL without network access or playlist resolution.
- `--check`: decode at least one second without an output device or reconnect retries.
- `--volume 0..100`: initial playback volume, default 50.
- `--tui`: show reception status, the current ICY title, and a session timer; `q`, Escape, or Ctrl-C stops playback.
- Ctrl-C also stops connection setup and ordinary CLI playback.

AAC/ADTS, MP3, and PCM WAV decoding are enabled. A station alias selects a stable
redirect endpoint; station availability and codec compatibility are external to
`rxer`. AAC support is limited to the profiles supported by Symphonia; this is not
a promise of universal AAC/HE-AAC compatibility.

## Configuration

Built-in stations live in [`assets/defaults.toml`](assets/defaults.toml) and are
embedded in the executable. Station-specific behavior does not live in Rust.
Create a `config.toml` to add or replace stations:

```toml
[stations.kusc]
name = "Classical California KUSC"
url = "https://96.aac.pls.kusc.live"

[stations.example]
url = "https://example.org/radio.m3u"
```

`url` is required; `name` is optional and defaults to the alias for display.
Aliases use lowercase ASCII letters, digits, `_`, and `-`. Unknown fields,
invalid URLs, and malformed TOML are errors. Future settings can extend this
format; there are no playback or network settings tables yet.

Configuration lookup, in priority order:

1. `--config PATH`
2. `RXER_CONFIG`
3. The platform default:
   - Linux/other Unix: `$XDG_CONFIG_HOME/rxer/config.toml`, or
     `$HOME/.config/rxer/config.toml` when XDG_CONFIG_HOME is unset or relative.
   - macOS: `$HOME/Library/Application Support/rxer/config.toml`
   - Windows: `%APPDATA%/rxer/config.toml`

The selected file overlays embedded defaults. Each user station replaces its
entire matching table; omitting `name` does not retain a built-in name. Other
built-in stations remain available. Files are never created automatically.
A missing platform-default file is fine; a missing explicit file or unreadable
or malformed file is an error. Help and version work without loading config.

```sh
rxer --config ./config.toml --list
rxer --config ./config.toml example
RXER_CONFIG=./config.toml rxer --resolve example
```

## Radio reception

PLS and M3U playlists are recognized by Content-Type or the final URL's extension,
including after HTTP redirects. Relative entries resolve against that final URL.
PLS selects the lowest-numbered usable `FileN`; M3U selects the first HTTP(S)
entry. Playlists are UTF-8, limited to 64 KiB each, with at most five resolution
requests and five HTTP redirects per request. Cycles and HLS tags are rejected.
Reconnect starts from the original configured URL and resolves playlists again.
There is no alternate-entry failover or HLS segment playback.

rxer requests ICY metadata and removes its blocks before decoding. `StreamTitle`
is shown in the CLI and TUI; control characters are removed and displayed titles
are capped at 512 characters. Metadata blocks are bounded by ICY's 4,080-byte
limit. Invalid UTF-8 uses replacement characters; legacy encodings are not
converted. HTTP responses with ICY headers are supported; legacy `ICY 200 OK`
status lines are not. Truncated metadata causes stream recovery.

Playback reconnects on EOF, read failures, or decoder failures. Input waits time
out after ten seconds of inactivity, without limiting healthy stream duration.
Five retries use delays of 1, 2, 4, 8, and 16 seconds. Decoding at least one second
resets the retry budget. An audio sample-rate or channel-count change ends the
session with an error. `--check` makes one attempt and checks initial decoding,
not long-term station health. Its decoded-audio wait has a 20-second deadline;
connection/probing happens before that deadline. A peer that continuously trickles
bytes can evade an idle timeout.

Reception, decoding, retries, and metadata handling run on one worker thread.
The PCM queue holds eight blocks of at most 2,048 frames. The worker holds at most
two additional blocks while priming; the audio source holds at most two while
playing or rebuffering. This bounds rxer's PCM staging to 24,576 frames, separate
from decoder, HTTP, and device buffers. Startup and reconnect prime two blocks;
starvation emits frame-aligned silence until two blocks arrive. The callback
uses nonblocking queue reads and performs no network I/O or metadata locking.
This is bounded best-effort playback, not a hard real-time allocation guarantee.

Ctrl-C stops the foreground session promptly, including during setup and retry
backoff. Dropping the receiver cancels the worker; pending network work finishes
at its timeout. Session time is wall-clock time, not received-audio duration.

Local-file playback, seeking, output selection, routing controls, and visualizers
remain outside this pass.

## Development

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo clippy --locked --all-targets --no-default-features -- -D warnings
cargo test --locked
cargo test --locked --no-default-features
```

Tests use synthetic local HTTP servers for playlists, ICY framing, idle timeouts,
EOF/stall recovery, and format-change rejection. Config tests isolate platform
paths in temporary directories. No audio device or live station is required.
CI runs both feature configurations on Linux, macOS, and Windows.

The new direct dependencies are Serde and TOML for configuration, and `url` for
URL validation and standards-based relative resolution. ureq is pinned because
the small idle-timeout adapter uses its unversioned transport API. Keep the project, crate, executable, and documentation name `rxer`.

## License

Licensed under [Apache 2.0](LICENSE).
Dependencies retain their own licenses; notably Symphonia uses MPL-2.0.
