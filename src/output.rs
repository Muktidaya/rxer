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
    format!("{time}    {status}    {artist}    {title}")
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
    fn rows_use_four_space_separators_without_padding_or_empty_tails() {
        let playing = row("04:05:06", "playing", "Hans Zimmer", "The Thin Red Line");
        assert_eq!(
            playing,
            "04:05:06    playing    Hans Zimmer    The Thin Red Line"
        );
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
