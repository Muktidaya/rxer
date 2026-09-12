//! Explicit selection of an existing local build; never invokes Cargo.
use std::{
    env,
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
};

fn options(args: Vec<OsString>) -> crate::Result<(Option<bool>, Vec<OsString>)> {
    let mut selected = None;
    let mut rest = Vec::new();
    let mut value = false;
    for arg in args {
        if value {
            rest.push(arg);
            value = false;
            continue;
        }
        let profile = if arg == "--dev" {
            Some(true)
        } else if arg == "--release" {
            Some(false)
        } else {
            None
        };
        if let Some(profile) = profile {
            if selected.is_some_and(|old| old != profile) {
                return Err("--dev and --release cannot be combined".into());
            }
            selected = Some(profile);
        } else {
            value = arg == "--config" || arg == "--volume";
            rest.push(arg);
        }
    }
    Ok((selected, rest))
}

fn destination(exe: &Path, explicit: Option<PathBuf>, dev: bool) -> Option<PathBuf> {
    let parent = exe.parent()?;
    let root = explicit.or_else(|| {
        matches!(parent.file_name()?.to_str()?, "debug" | "release")
            .then(|| parent.parent().map(Path::to_path_buf))
            .flatten()
    })?;
    Some(
        root.join(if dev { "debug" } else { "release" })
            .join(format!("rxer{}", env::consts::EXE_SUFFIX)),
    )
}

pub fn select(args: Vec<OsString>) -> crate::Result<Vec<OsString>> {
    let (profile, args) = options(args)?;
    let Some(dev) = profile else {
        return Ok(args);
    };
    let exe = env::current_exe()?.canonicalize()?;
    let target = destination(&exe, env::var_os("RXER_BUILD_DIR").map(PathBuf::from), dev);
    let Some(target) = target else {
        if dev == cfg!(debug_assertions) {
            return Ok(args);
        }
        return Err("no alternate build location; set RXER_BUILD_DIR to the directory containing debug/ and release/".into());
    };
    let target = target.canonicalize().map_err(|e| {
        format!(
            "cannot open {}: {e}; build it with cargo build{}",
            target.display(),
            if dev { "" } else { " --release" }
        )
    })?;
    if target == exe {
        return Ok(args);
    }
    let mut command = Command::new(target);
    command.args(&args);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        Err(command.exec().into())
    }
    #[cfg(not(unix))]
    {
        std::process::exit(command.status()?.code().unwrap_or(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }
    #[test]
    fn selection_preserves_option_values_and_rejects_conflicts() {
        assert_eq!(
            options(args(&["--config", "--dev", "--release", "--help"])).unwrap(),
            (Some(false), args(&["--config", "--dev", "--help"]))
        );
        assert!(options(args(&["--dev", "--release"])).is_err());
        assert_eq!(
            options(args(&["--dev", "--dev", "kusc"])).unwrap(),
            (Some(true), args(&["kusc"]))
        );
    }
    #[test]
    fn finds_sibling_or_explicit_build_without_using_working_directory() {
        let name = format!("rxer{}", env::consts::EXE_SUFFIX);
        assert_eq!(
            destination(Path::new("target/release/rxer"), None, true),
            Some(Path::new("target/debug").join(&name))
        );
        assert_eq!(
            destination(Path::new("bin/rxer"), Some(PathBuf::from("builds")), false),
            Some(Path::new("builds/release").join(name))
        );
        assert_eq!(destination(Path::new("bin/rxer"), None, true), None);
    }
}
