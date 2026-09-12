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
    text.chars().fold(String::new(), |mut out, c| {
        match c {
            '\\' => out.push_str("\\\\"),
            '|' => out.push_str("\\|"),
            c if c.is_control() => out.push(' '),
            c => out.push(c),
        }
        out
    })
}

fn row(time: &str, status: &str, artist: &str, title: &str) -> String {
    format!(
        "| {time} | {} | {} | {} |",
        field(status),
        field(artist),
        field(title)
    )
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
    fn rows_keep_untrusted_metadata_on_one_line() {
        assert_eq!(
            row("04:05:06", "playing", "A|B", "Work\nnext"),
            "| 04:05:06 | playing | A\\|B | Work next |"
        );
    }
}
