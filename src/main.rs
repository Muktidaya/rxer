use std::{
    env,
    process::ExitCode,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};
mod audio;
mod config;
mod radio;
#[cfg(test)]
#[path = "../tests/support/mod.rs"]
mod test_support;

const HELP: &str = "rxer — a lean terminal audio receiver and router

Usage: rxer [--tui] [--volume 0..100] [--config PATH] <URL|alias>
       rxer --resolve <URL|alias>
       rxer --list

Plays an HTTP(S) audio stream or PLS/M3U/HLS playlist with Rust-native decoding and audio output.
--tui          Show a Ratatui session display; q or Esc stops playback
--volume N     Initial volume (default: 50)
--check        Decode one second without opening an audio device
--resolve      Print the stream URL without starting playback
--list         List effective station aliases
--config PATH  Use this TOML file (overrides RXER_CONFIG and platform default)
-h, --help     Show help
-V, --version  Show version

Ctrl-C stops playback. No audio visualization or routing controls yet.";

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Debug, PartialEq)]
enum Action {
    Help,
    Version,
    List {
        config: Option<std::path::PathBuf>,
    },
    Play {
        url: String,
        config: Option<std::path::PathBuf>,
        tui: bool,
        volume: u8,
        resolve: bool,
        check: bool,
    },
}

fn parse(args: impl IntoIterator<Item = String>) -> Result<Action> {
    let mut args = args.into_iter();
    let mut check = false;
    let mut config = None;
    let mut list = false;
    let (mut tui, mut resolve_only, mut volume, mut source) = (false, false, 50, None);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(Action::Help),
            "-V" | "--version" => return Ok(Action::Version),
            "--list" => list = true,
            "--config" => config = Some(args.next().ok_or("--config requires a path")?.into()),
            "--tui" => tui = true,
            "--check" => check = true,
            "--resolve" => resolve_only = true,
            "--volume" => {
                volume = args
                    .next()
                    .ok_or("--volume requires a number from 0 to 100")?
                    .parse::<u8>()?;
                if volume > 100 {
                    return Err("volume must be between 0 and 100".into());
                }
            }
            _ if arg.starts_with('-') => return Err(format!("unknown option: {arg}").into()),
            _ => {
                if source.replace(arg).is_some() {
                    return Err("provide exactly one stream URL or alias".into());
                }
            }
        }
    }
    if list {
        if source.is_some() {
            return Err("--list does not accept a source".into());
        }
        return Ok(Action::List { config });
    }
    let source = source.ok_or("provide a stream URL or alias; try rxer --help")?;
    Ok(Action::Play {
        url: source,
        config,
        tui,
        volume,
        resolve: resolve_only,
        check,
    })
}

fn play(
    url: &str,
    volume: u8,
    tui: bool,
    check: bool,
    encoding: Option<&'static encoding_rs::Encoding>,
) -> Result<()> {
    #[cfg(not(feature = "tui"))]
    if tui {
        return Err("this build has no TUI; rebuild with the default features".into());
    }
    #[cfg(feature = "tui")]
    if tui {
        use std::io::IsTerminal;
        if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
            return Err("--tui requires an interactive terminal".into());
        }
    }
    let stopped = Arc::new(AtomicBool::new(false));
    let signal = stopped.clone();
    ctrlc::set_handler(move || signal.store(true, Ordering::Relaxed))?;
    if !check {
        eprintln!("rxer: connecting; Ctrl-C to stop");
    }
    let Some(receiver) = audio::connect_encoded(url, &stopped, !check, encoding)? else {
        return Ok(());
    };
    if check {
        return receiver.check(&stopped);
    }
    let mut device = rodio::DeviceSinkBuilder::open_default_sink()?;
    device.log_on_drop(false);
    let player = rodio::Player::connect_new(device.mixer());
    player.set_volume(f32::from(volume) / 100.0);
    let failure = receiver.failure.clone();
    let status = receiver.status.clone();
    player.append(receiver);
    if tui {
        #[cfg(feature = "tui")]
        tui_loop(&player, &stopped, &status)?;
    } else {
        let mut previous = radio::Status::default();
        while !player.empty() && !stopped.load(Ordering::Relaxed) {
            if let Ok(state) = status.lock()
                && *state != previous
            {
                eprintln!(
                    "rxer: {}{}",
                    state.message,
                    if state.title.is_empty() {
                        String::new()
                    } else {
                        format!(" — {}", state.title)
                    }
                );
                previous = state.clone();
            }
            thread::sleep(Duration::from_millis(100));
        }
    }
    player.stop();
    if !stopped.load(Ordering::Relaxed)
        && let Some(error) = failure.lock().map_err(|_| "audio worker failed")?.take()
    {
        return Err(error.into());
    }
    Ok(())
}

#[cfg(feature = "tui")]
fn tui_loop(
    player: &rodio::Player,
    stopped: &AtomicBool,
    status: &radio::SharedStatus,
) -> Result<()> {
    use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
    use ratatui::widgets::{Block, Paragraph};
    struct Restore;
    impl Drop for Restore {
        fn drop(&mut self) {
            ratatui::restore();
        }
    }
    let _restore = Restore;
    let mut terminal = ratatui::try_init()?;
    let started = std::time::Instant::now();
    while !player.empty() && !stopped.load(Ordering::Relaxed) {
        let state = status.lock().map_err(|_| "status unavailable")?.clone();
        terminal.draw(|frame| {
            let text = format!("{}\n{}\nSession: {} s\n\nq / Esc / Ctrl-C: stop\n\nSession time is not a signal meter.", state.message, state.title, started.elapsed().as_secs());
            frame.render_widget(Paragraph::new(text).block(Block::bordered().title("rxer")), frame.area());
        })?;
        if event::poll(Duration::from_millis(100))?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
            && (matches!(key.code, KeyCode::Char('q') | KeyCode::Esc)
                || (key.code == KeyCode::Char('c')
                    && key.modifiers.contains(KeyModifiers::CONTROL)))
        {
            stopped.store(true, Ordering::Relaxed);
            break;
        }
    }
    Ok(())
}

fn run() -> Result<()> {
    match parse(env::args().skip(1))? {
        Action::Help => println!("{HELP}"),
        Action::Version => println!("rxer {}", env!("CARGO_PKG_VERSION")),
        Action::List { config } => {
            for (alias, station) in config::Config::load(config)?.stations {
                println!(
                    "{alias}\t{}\t{}",
                    station.name.as_deref().unwrap_or(&alias),
                    station.url
                );
            }
        }
        Action::Play {
            url,
            config,
            tui,
            volume,
            resolve,
            check,
        } => {
            let config = config::Config::load(config)?;
            let encoding = config
                .stations
                .get(&url)
                .and_then(|s| s.metadata_encoding.as_deref())
                .and_then(|s| encoding_rs::Encoding::for_label(s.as_bytes()));
            let url = config.resolve(&url)?;
            if resolve {
                println!("{url}");
            } else {
                play(&url, volume, tui, check, encoding)?;
            }
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("rxer: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn args(s: &[&str]) -> Result<Action> {
        parse(s.iter().map(|s| s.to_string()))
    }
    #[test]
    fn validates_inputs_before_spawning() {
        for bad in [
            vec![],
            vec!["--volume", "101", "kusc"],
            vec!["--volume"],
            vec!["--unknown"],
            vec!["kusc", "kusc"],
        ] {
            assert!(args(&bad).is_err(), "{bad:?}");
        }
    }
    #[test]
    fn resolves_alias_and_options() {
        assert_eq!(
            args(&["--volume", "0", "kusc", "--tui", "--resolve"]).unwrap(),
            Action::Play {
                url: "kusc".into(),
                config: None,
                volume: 0,
                tui: true,
                resolve: true,
                check: false
            }
        );
    }
}
