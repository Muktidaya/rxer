use std::{fs, process::Command};

#[test]
fn explicit_build_selection_errors_and_same_executable() {
    let binary = env!("CARGO_BIN_EXE_rxer");
    let root = std::env::temp_dir().join(format!("rxer-build-selection-{}", std::process::id()));
    fs::create_dir_all(root.join("debug")).unwrap();
    let copy = root
        .join("debug")
        .join(format!("rxer{}", std::env::consts::EXE_SUFFIX));
    fs::copy(binary, &copy).unwrap();
    let output = Command::new(&copy)
        .args(["--dev", "--help"])
        .env_remove("RXER_BUILD_DIR")
        .current_dir(std::env::temp_dir())
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("--config"));
    let output = Command::new(&copy)
        .args(["--release", "--help"])
        .env_remove("RXER_BUILD_DIR")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("cargo build --release"));
    let output = Command::new(&copy)
        .args(["--dev", "--release", "--help"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn dispatch_preserves_arguments_working_directory_and_exit_status() {
    use std::os::unix::fs::PermissionsExt;
    let root = std::env::temp_dir().join(format!("rxer-build-dispatch-{}", std::process::id()));
    fs::create_dir_all(root.join("release")).unwrap();
    let target = root.join("release/rxer");
    fs::write(
        &target,
        "#!/bin/sh\nprintf '%s\\n' \"$PWD\" \"$@\"\nexit 23\n",
    )
    .unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o755)).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_rxer"))
        .args(["--release", "--config", "a file.toml", "--help"])
        .env("RXER_BUILD_DIR", &root)
        .current_dir(&root)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(23));
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(
        text.ends_with("\n--config\na file.toml\n--help\n"),
        "{text}"
    );
    assert_eq!(
        PathBuf::from(text.lines().next().unwrap())
            .canonicalize()
            .unwrap(),
        root.canonicalize().unwrap()
    );
    fs::remove_dir_all(root).unwrap();
}
#[cfg(unix)]
use std::path::PathBuf;
