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
    [time, status, artist, title].map(field).join("\t")
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
    fn rows_use_tabs_and_keep_metadata_in_its_field() {
        assert_eq!(
            row("04:05:06", "playing", "A|B", "Work\nnext\tmovement"),
            "04:05:06\tplaying\tA|B\tWork next movement"
        );
        assert_eq!(row("04:05:06", "playing", "", ""), "04:05:06\tplaying\t\t");
        assert_eq!(
            row("04:05:06", "playing", "Artist", "A, \"quoted\" title"),
            "04:05:06\tplaying\tArtist\tA, \"quoted\" title"
        );
    }
}
