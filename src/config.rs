use crate::Result;
use serde::Deserialize;
use std::{collections::BTreeMap, env, fs, path::PathBuf};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Station {
    pub url: String,
    pub name: Option<String>,
    pub metadata_encoding: Option<String>,
}
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub stations: BTreeMap<String, Station>,
}
impl Config {
    pub fn defaults() -> Result<Self> {
        Self::merge(include_str!("../assets/defaults.toml"), None)
    }
    fn merge(defaults: &str, user: Option<&str>) -> Result<Self> {
        let mut config: Self = toml::from_str(defaults)?;
        if let Some(user) = user {
            let user: Self = toml::from_str(user)?;
            config.stations.extend(user.stations);
        }
        for (alias, station) in &config.stations {
            if alias.is_empty()
                || !alias
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
            {
                return Err(format!(
                    "invalid station alias: {alias:?}; use lowercase letters, digits, _ or -"
                )
                .into());
            }
            if let Some(label) = &station.metadata_encoding
                && encoding_rs::Encoding::for_label(label.as_bytes()).is_none()
            {
                return Err(format!("station {alias}: unknown metadata encoding {label:?}").into());
            }
            validate_url(&station.url).map_err(|e| format!("station {alias}: {e}"))?;
            if station
                .name
                .as_ref()
                .is_some_and(|n| n.chars().any(char::is_control))
            {
                return Err(format!("station {alias}: name contains control characters").into());
            }
        }
        Ok(config)
    }
    pub fn load(explicit: Option<PathBuf>) -> Result<Self> {
        let explicit = explicit.or_else(|| env::var_os("RXER_CONFIG").map(PathBuf::from));
        let required = explicit.is_some();
        let Some(path) = explicit.or_else(default_path) else {
            return Self::defaults();
        };
        match fs::read_to_string(&path) {
            Ok(text) => Self::merge(include_str!("../assets/defaults.toml"), Some(&text))
                .map_err(|e| format!("config {}: {e}", path.display()).into()),
            Err(e) if !required && e.kind() == std::io::ErrorKind::NotFound => Self::defaults(),
            Err(e) => Err(format!("config {}: {e}", path.display()).into()),
        }
    }
    pub fn resolve(&self, input: &str) -> Result<String> {
        let value = self.stations.get(input).map_or(input, |s| s.url.as_str());
        validate_url(value)?;
        Ok(value.into())
    }
}
fn default_path() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    let base = env::var_os("APPDATA").map(PathBuf::from);
    #[cfg(target_os = "macos")]
    let base = env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support"));
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let base = env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")));
    base.map(|p| p.join("rxer/config.toml"))
}
pub fn validate_url(input: &str) -> Result<url::Url> {
    let url = url::Url::parse(input)
        .map_err(|_| "expected an HTTP(S) URL or known station alias; try rxer --list")?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || input.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return Err(
            "expected an HTTP(S) URL with a host and no whitespace or control characters".into(),
        );
    }
    Ok(url)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn overlays_whole_station_tables_and_preserves_other_defaults() {
        let c = Config::merge("[stations.a]\nurl='https://example.org/a'\nname='old'\n[stations.b]\nurl='https://example.org/b'",
            Some("[stations.a]\nurl='https://example.org/new'\n[stations.c]\nurl='http://example.org/c'")).unwrap();
        assert_eq!(c.stations.len(), 3);
        assert_eq!(c.resolve("a").unwrap(), "https://example.org/new");
        assert!(c.stations["a"].name.is_none());
    }
    #[test]
    fn rejects_invalid_configuration() {
        for text in [
            "[stations.a]\nname='missing url'",
            "[stations.a]\nurl='file:///x'",
            "[stations.UPPER]\nurl='https://example.org'",
            "[stations.a]\nurl='https://example.org'\nurll='typo'",
            "[stations.a]\nurl='https://example.org'\nmetadata_encoding='unknown-encoding'",
            "invalid toml",
        ] {
            assert!(Config::merge("", Some(text)).is_err(), "{text}");
        }
    }
}
