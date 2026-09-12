//! Human-readable CLI events on stderr; command results stay on stdout.
use chrono::{Local, Timelike};

pub fn track(metadata: &str) -> (&str, &str) {
    for separator in [" - ", " – ", " — "] {
        if let Some((artist, title)) = metadata.split_once(separator)
            && !artist.trim().is_empty()
            && !title.trim().is_empty()
        {
            return (artist.trim(), title.trim());
        }
    }
    ("", metadata.trim())
}

fn field(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

fn row(time: &str, status: &str, artist: &str, title: &str) -> String {
    let [time, status, artist, title] = [time, status, artist, title].map(field);
    format!("{time:8}    {status:12}    {artist:28}    {title}")
        .trim_end()
        .to_owned()
}

pub fn event(status: &str, artist: &str, title: &str) {
    let now = Local::now();
    let time = format!("{:02}:{:02}:{:02}", now.hour(), now.minute(), now.second());
    eprintln!("{}", row(&time, status, artist, title));
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_titles_and_handles_missing_or_ambiguous_metadata() {
        assert_eq!(
            track("Ignace Pleyel - Rondo in Bb"),
            ("Ignace Pleyel", "Rondo in Bb")
        );
        assert_eq!(
            track("Artist - Work - Movement"),
            ("Artist", "Work - Movement")
        );
        assert_eq!(track("Artist — Work"), ("Artist", "Work"));
        assert_eq!(track("Untitled"), ("", "Untitled"));
        assert_eq!(track(""), ("", ""));
    }
    #[test]
    fn rows_align_status_and_artist_without_tabs_or_empty_tails() {
        let playing = row("04:05:06", "playing", "Hans Zimmer", "The Thin Red Line");
        let connecting = row("04:05:06", "connecting", "Hans Zimmer", "The Thin Red Line");
        assert_eq!(playing.find("Hans Zimmer"), connecting.find("Hans Zimmer"));
        assert_eq!(playing.find("The Thin Red Line"), Some(60));
        assert!(!playing.contains('\t'));
        assert_eq!(
            row("04:05:06", "connecting", "", ""),
            "04:05:06    connecting"
        );
        assert!(
            row("04:05:06", "playing", "A|B", "Work\nnext\tmovement")
                .ends_with("Work next movement")
        );
        assert!(
            row("04:05:06", "playing", "Artist", "A, \"quoted\" title")
                .ends_with("A, \"quoted\" title")
        );
    }
}
