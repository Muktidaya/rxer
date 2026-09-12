# rxer

a lean terminal audio receiver and router

`rxer` 0.1.0 starts with internet radio: receive a direct HTTP(S) audio stream,
decode it in Rust, and play it through the default audio output. An optional
Ratatui session display keeps the terminal interface small.

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

- `--list`: list built-in station aliases; initially `kusc`.
- `--resolve`: print the alias's stream URL without making a network request.
- `--check`: decode at least one second of audio without opening an output device.
- `--volume 0..100`: initial playback volume, default 50.
- `--tui`: show a session timer; `q`, Escape, or Ctrl-C stops playback.
- Ctrl-C also stops connection setup and ordinary CLI playback.

AAC/ADTS, MP3, and PCM WAV decoding are enabled. A station alias selects a stable
redirect endpoint; station availability and codec compatibility are external to
`rxer`. AAC support is limited to the profiles supported by Symphonia; this is not
a promise of universal AAC/HE-AAC compatibility.

The first release supports one stream and the default output device. General
playlist parsing (PLS/M3U/HLS), inline ICY metadata, reconnection, seeking, output
selection, routing controls, and audio visualizations are not implemented. Supply
a direct audio URL, not a station web page. Local files are not accepted yet.

Reception and decoding run on a worker thread. The PCM queue is bounded to eight
blocks of 2,048 frames plus the current block. Temporary starvation produces
silence without blocking the audio callback. A stalled connection must be stopped
manually; automatic recovery is future work. Session time is wall-clock time,
not a measure of audio received. The decoder can treat some malformed/truncated
streams as end-of-stream; `--check` establishes initial decoding, not stream health.

## Development

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --locked
cargo test --locked --no-default-features
```

HTTP tests serve synthetic audio locally and do not need an audio device or a
live station. Keep the project, crate, executable, and documentation name `rxer`.

## License

Copyright 2026 Muktidaya. Licensed under [Apache 2.0](LICENSE).
Dependencies retain their own licenses; notably Symphonia uses MPL-2.0.
