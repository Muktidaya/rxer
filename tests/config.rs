use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
};
static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "rxer-config-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_rxer"));
        command
            .env_remove("RXER_CONFIG")
            .env("HOME", &self.0)
            .env("APPDATA", &self.0)
            .env("XDG_CONFIG_HOME", &self.0);
        command
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[test]
fn default_paths_overrides_and_list_option_order() {
    let temp = Temp::new();
    let defaults = temp.command().args(["--resolve", "kusc"]).output().unwrap();
    assert!(defaults.status.success());
    #[cfg(target_os = "macos")]
    let base = temp.0.join("Library/Application Support");
    #[cfg(not(target_os = "macos"))]
    let base = temp.0.clone();
    let default = base.join("rxer/config.toml");
    fs::create_dir_all(default.parent().unwrap()).unwrap();
    fs::write(
        &default,
        "[stations.custom]\nurl='https://example.org/default'",
    )
    .unwrap();
    let out = temp
        .command()
        .args(["--resolve", "custom"])
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "https://example.org/default"
    );
    let custom = temp.0.join("custom.toml");
    fs::write(
        &custom,
        "[stations.kusc]\nurl='https://example.org/replacement'",
    )
    .unwrap();
    let out = temp
        .command()
        .env("RXER_CONFIG", &custom)
        .args(["--resolve", "kusc"])
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "https://example.org/replacement"
    );
    for args in [vec!["--list", "--config"], vec!["--config"]] {
        let mut cmd = temp.command();
        cmd.env("RXER_CONFIG", temp.0.join("missing"))
            .args(&args)
            .arg(&custom);
        if args.len() == 1 {
            cmd.arg("--list");
        }
        let out = cmd.output().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(String::from_utf8_lossy(&out.stdout).contains("https://example.org/replacement"));
        assert!(!String::from_utf8_lossy(&out.stdout).contains("custom"));
    }
}
#[test]
fn explicit_missing_invalid_files_and_unknown_sources_fail() {
    let temp = Temp::new();
    assert!(
        !temp
            .command()
            .env("RXER_CONFIG", temp.0.join("missing"))
            .arg("--list")
            .output()
            .unwrap()
            .status
            .success()
    );
    let path = temp.0.join("bad.toml");
    fs::write(&path, "[stations.bad]\nname='no URL'").unwrap();
    assert!(
        !temp
            .command()
            .arg("--config")
            .arg(path)
            .arg("--list")
            .output()
            .unwrap()
            .status
            .success()
    );
    for source in [
        "unknown",
        "file:///tmp/test",
        "https://",
        "https://example.org/\n",
    ] {
        assert!(
            !temp
                .command()
                .args(["--resolve", source])
                .output()
                .unwrap()
                .status
                .success()
        );
    }
    assert!(
        temp.command()
            .env("RXER_CONFIG", temp.0.join("missing"))
            .arg("--help")
            .output()
            .unwrap()
            .status
            .success()
    );
}
