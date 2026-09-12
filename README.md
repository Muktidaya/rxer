# rxer

An audio receiver and router.

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

## Selecting a local build

Ordinary `rxer` runs the executable on your PATH. `--release` and `--dev`
select an already-built release or debug executable and forward the remaining
arguments. They never compile or update a build.

```sh
cargo build --locked
cargo build --release --locked
rxer --dev --tui kusc
rxer --release --tui kusc
```

For checkout builds, the flags find sibling `debug/rxer` and `release/rxer`
executables beneath the same build directory (including `.exe` on Windows).
Developers can put a symlink to `target/release/rxer` on PATH to default to
that checkout's release build from any directory. Rebuild with
`cargo build --release` to update it; ordinary `cargo build` updates only debug.

For an installed executable, selecting its own profile is a no-op. To select
checkout builds from an installed copy, set `RXER_BUILD_DIR` to the absolute
build directory containing `debug/` and `release/` (for cross-compilation,
the directory beneath the target triple). Missing builds produce an error.
The flags cannot be combined. Neither flag changes the default for later runs.

## Controls and scope

- `--list`: list effective station aliases, names, and URLs.
- `--config PATH`: choose a TOML configuration file.
- `--resolve`: print the configured URL without network access or playlist resolution.
- `--check`: decode at least one second without an output device or delayed reconnects;
  playlist alternatives are still tried.
- `--volume 0..100`: linear amplitude percentage, default 100% (unity gain, 0 dB).
- `--tui`: show reception status, the current ICY title, and a session timer; Ctrl-C stops playback.
- Ctrl-C is the only quit key, in both the TUI and ordinary CLI; it also stops connection setup.
  `q` and Escape do not stop playback.

AAC/ADTS, MP3, PCM WAV, and AAC-in-MP4 decoding are enabled. A station alias selects a stable
redirect endpoint; station availability and codec compatibility are external to
`rxer`. AAC support is limited to the profiles supported by Symphonia; this is not
a promise of universal AAC/HE-AAC compatibility.

## CLI output

Playback events and errors go to stderr as local-clock `HH:MM:SS` rows:

```text
04:05:06	connecting		Ctrl-C to stop
04:05:08	playing	Ignace Pleyel	Rondo in Bb
```

Columns are time, status, artist, and title. A timestamp records when rxer
reports an event, not an exact audible track boundary. Metadata is split at the
first ` - ` separator (also accepting spaced en/em dashes); without a separator,
the entire text stays in the title column. This is a display convention, since
ICY titles do not guarantee structured artist/title fields. Empty fields stay
empty. Fields are separated by tabs, with no added quotes or borders.
Control characters in metadata (including tabs and newlines) become spaces;
punctuation remains literal. Tab spacing follows the terminal's tab stops.

Updates print only when status or metadata changes. `--help`, `--list`,
`--resolve`, and `--check` results retain their existing stdout formats.
The TUI retains its own display.

## Gain staging

The default volume is 100%: a linear amplitude multiplier of 1.0, or **0 dB
of gain** at rxer's playback volume stage. This preserves the incoming level
at that stage. It does not normalize loudness, push peaks to 0 dBFS, or set
system/device volume. Quieter streams remain quieter.

`--volume` only attenuates: 50% is a multiplier of 0.5 (approximately -6.02 dB),
25% is approximately -12.04 dB, and 0% mutes. Values above 100 are rejected.
The app adds no automatic gain control, loudness normalization, or limiter.
Sample-rate/channel conversion and the system's output processing are separate;
unity volume is not a claim of bit-perfect output or guaranteed clipping safety.

Future EQ and other DSP must explicitly account for boosts and peak headroom,
with any preattenuation or output protection documented and controllable.
Bypassed processing should retain unity gain. A default 0 dB volume setting
alone cannot prevent clipping from boosted filters or an overloaded source.
These are design requirements for future processing; no EQ or limiter is
implemented in this pass.

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
`metadata_encoding` optionally selects a legacy ICY text encoding, for example:

```toml
[stations.example]
url = "https://example.org/radio"
metadata_encoding = "windows-1251"
```

Encoding labels follow the Encoding Standard (`utf-8`, `windows-1252`,
`shift_jis`, and so on); unknown labels are rejected.
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
PLS tries `FileN` entries in numeric order; M3U tries entries in file order.
Connection, read, and decoder failures advance to another entry. Retries rotate
entry preference so one broken endpoint does not permanently monopolize playback.
Playlists are UTF-8, limited to 64 KiB each and 64 alternatives, with a maximum
nesting depth of five and 32 resolution requests per reconnect attempt. Each
request permits five HTTP redirects. Cycles are rejected. Playlist reads check
a 30-second deadline between reads, also subject to the ten-second idle limit.

HLS supports master and media playlists, audio rendition groups, relative URIs,
VOD completion, live reloads, sequence tracking, discontinuities, gap tags, byte
ranges, initialization maps, and identity AES-128 encryption (explicit IV or
media-sequence IV). Supported segment containers are packed AAC/MP3, MPEG-TS
with ADTS or MPEG audio, and fMP4 with supported audio codecs. Other TS tracks
are ignored. Fresh live sessions start at most three segments behind the edge;
segments that have left the server's window are skipped. Reconnect retains the
last active media playlist's sequence position, so failed fetches retry without
replaying completed segments. A reset sequence starts a new live window.

Master selection prefers variants without a declared video resolution, then
lower bandwidth. Associated audio renditions are tried first, with the default
rendition preferred. This is deterministic selection and failover; there is no
continuous adaptive-bitrate controller or language-selection interface.

HLS downloads one segment at a time, capped at 16 MiB, plus an initialization map
capped at 1 MiB. Keys must be exactly 16 bytes. Segment/map/key body reception has
a 30-second total deadline as well as the idle timeout. Byte-range responses
must match the requested range. Decryption is in place; TS extraction and map
assembly can temporarily retain both input and output encoded buffers. HLS
therefore adds bounded encoded staging, separate from the PCM queue below.
No external player, downloader, or FFmpeg process runs during playback.

SAMPLE-AES/DRM, low-latency/delta HLS, I-frame-only playlists, video playback,
unsupported audio codecs (including AC-3 and LATM AAC), and sample-accurate
continuity across independently decoded segments are outside this implementation.
Malformed or unsupported media advances through alternatives or ends with an
error after the retry budget. This receiver follows the conventional delivery
model in [RFC 8216](https://www.rfc-editor.org/rfc/rfc8216).

rxer accepts ordinary HTTP and legacy `ICY 200 OK` responses, including fragmented
status lines. It requests ICY metadata and removes its blocks before decoding.
`StreamTitle` is shown in the CLI and TUI; control characters are removed and
displayed titles are capped at 512 characters. Metadata blocks are bounded by
ICY's 4,080-byte limit. Text uses the station's `metadata_encoding`, then a
recognized Content-Type charset, then valid UTF-8 or Windows-1252 fallback.
Unknown source bytes can still produce replacement characters. Truncated metadata
causes stream recovery. HLS timed ID3 metadata is not displayed.

Playback reconnects on EOF, read failures, or decoder failures. Input waits time
out after ten seconds of inactivity, without limiting healthy stream duration.
Five retries use delays of 1, 2, 4, 8, and 16 seconds. Decoding at least one second
resets the retry budget. Successful HLS ENDLIST playback finishes normally.

The first decoded audio establishes the session's PCM rate and channel count.
Later formats are converted to it using rodio's resampler/channel converter on
the worker. The source adapter accounts for empty MP3 priming frames and decoder
packet boundaries. The device callback never handles format changes or decoding.
This does not promise high-end resampling or artifact-free channel conversion.

`--check` makes one resolution/failover pass and checks initial decoding, not
long-term station health. Its decoded-audio wait has a 20-second deadline;
connection/probing happens before that deadline. A direct audio peer that
continuously trickles bytes can evade the idle timeout during probing.

Reception, decoding, retries, and metadata handling run on one worker thread.
The PCM queue holds eight blocks of at most 2,048 frames. The worker holds at most
two additional blocks while priming; the audio source holds at most two while
playing or rebuffering. This bounds rxer's PCM block staging to 24,576 frames, separate
from decoder/resampler lookahead, HTTP, and device buffers. Startup and reconnect prime two blocks;
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

Tests use synthetic local HTTP servers for playlists, ICY framing/encodings, idle
timeouts, format conversion, HLS container decoding, AES-128, byte ranges, live
sequence tracking, and reconnect recovery. Config tests isolate platform
paths in temporary directories. No audio device or live station is required.
CI runs both feature configurations on Linux, macOS, and Windows.

Serde/TOML handle configuration and `url` handles relative resolution.
`encoding_rs` handles text encodings, `mpeg2ts-reader` handles TS demultiplexing,
and RustCrypto `aes`/`cbc` handle AES-128 segments. fMP4 uses the existing
Symphonia stack. ureq is pinned for its unversioned transport API; rodio is pinned
for the decoder span adapter. Synthetic codec fixtures and their generation
commands are in [`tests/fixtures`](tests/fixtures/README.md). Keep the project, crate, executable, and documentation name `rxer`.

## License

Licensed under [Apache 2.0](LICENSE).
Dependencies retain their own licenses; notably Symphonia uses MPL-2.0.
