//! Bounded playlist and ICY parsing; all work happens on the decoder thread.
use crate::{Result, config::validate_url};
use std::{
    io::{self, Read},
    sync::{Arc, Mutex},
};
use ureq::ResponseExt;
mod hls;
mod transport;
pub use transport::agent;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Status {
    pub message: String,
    pub title: String,
}
pub type SharedStatus = Arc<Mutex<Status>>;

const MAX_PLAYLIST: u64 = 64 * 1024;

fn read_playlist(mut reader: impl Read, active: &dyn Fn() -> bool) -> Result<String> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let mut bytes = Vec::new();
    let mut buffer = [0; 4096];
    loop {
        if !active() {
            return Err("playlist reception cancelled".into());
        }
        if std::time::Instant::now() >= deadline {
            return Err("playlist reception timed out".into());
        }
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        if bytes.len() + count > MAX_PLAYLIST as usize {
            return Err("playlist exceeds 64 KiB".into());
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
    String::from_utf8(bytes).map_err(|_| "playlist must be UTF-8".into())
}

pub trait MediaRead: Read + std::io::Seek + Send + Sync {}
impl<T: Read + std::io::Seek + Send + Sync> MediaRead for T {}
pub struct Input {
    pub data: Box<dyn MediaRead>,
    pub mime: String,
    pub interval: Option<usize>,
    pub finite: bool,
    pub byte_len: Option<u64>,
}
struct Unseekable<R>(R);
impl<R: Read> Read for Unseekable<R> {
    fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
        self.0.read(b)
    }
}
impl<R> std::io::Seek for Unseekable<R> {
    fn seek(&mut self, _: std::io::SeekFrom) -> io::Result<u64> {
        Err(io::ErrorKind::Unsupported.into())
    }
}
pub enum Session {
    Direct(Option<Input>),
    Hls(Box<hls::Session>),
}
impl Session {
    pub fn checkpoint(&self) -> Option<(url::Url, u64)> {
        match self {
            Self::Hls(hls) => Some(hls.checkpoint()),
            Self::Direct(_) => None,
        }
    }
    pub fn resume(&mut self, checkpoint: Option<&(url::Url, u64)>) {
        if let (Self::Hls(hls), Some(checkpoint)) = (self, checkpoint) {
            hls.resume(checkpoint);
        }
    }
    pub fn next(
        &mut self,
        agent: &ureq::Agent,
        active: &dyn Fn() -> bool,
    ) -> Result<Option<Input>> {
        match self {
            Self::Direct(input) => Ok(input.take()),
            Self::Hls(hls) => hls.next(agent, active),
        }
    }
}
pub struct Resolver {
    pending: Vec<(url::Url, usize)>,
    visited: std::collections::HashSet<String>,
    requests: usize,
    rotation: usize,
    last_error: Option<String>,
}
impl Resolver {
    pub fn new(input: &str, rotation: usize) -> Result<Self> {
        Ok(Self {
            pending: vec![(validate_url(input)?, 0)],
            visited: Default::default(),
            requests: 0,
            rotation,
            last_error: None,
        })
    }
    pub fn next(
        &mut self,
        agent: &ureq::Agent,
        active: &dyn Fn() -> bool,
    ) -> Result<Option<Session>> {
        while let Some((url, depth)) = self.pending.pop() {
            if !active() {
                return Ok(None);
            }
            if depth >= 5 {
                self.last_error = Some("playlist nesting depth exceeded".into());
                continue;
            }
            if self.requests >= 32 {
                return Err("playlist resolution budget exceeded".into());
            }
            if !self.visited.insert(url.to_string()) {
                self.last_error = Some("playlist cycle or repeated entry".into());
                continue;
            }
            self.requests += 1;
            let attempt = (|| -> Result<Option<Session>> {
                let response = agent.get(url.as_str()).header("Icy-MetaData", "1").call()?;
                let base = validate_url(&response.get_uri().to_string())?;
                let mime = response
                    .headers()
                    .get("content-type")
                    .and_then(|h| h.to_str().ok())
                    .unwrap_or("")
                    .to_string();
                let kind = mime
                    .split(';')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .to_ascii_lowercase();
                let path = base.path().to_ascii_lowercase();
                let pls = kind == "audio/x-scpls" || path.ends_with(".pls");
                let m3u = matches!(
                    kind.as_str(),
                    "audio/x-mpegurl"
                        | "audio/mpegurl"
                        | "application/x-mpegurl"
                        | "application/vnd.apple.mpegurl"
                ) || path.ends_with(".m3u")
                    || path.ends_with(".m3u8");
                if !pls && !m3u {
                    let interval = response
                        .headers()
                        .get("icy-metaint")
                        .map(|h| -> Result<usize> { Ok(h.to_str()?.parse()?) })
                        .transpose()?;
                    return Ok(Some(Session::Direct(Some(Input {
                        data: Box::new(Unseekable(response.into_body().into_reader())),
                        mime,
                        interval,
                        finite: false,
                        byte_len: None,
                    }))));
                }
                let text = read_playlist(response.into_body().into_reader(), active)?;
                let mut entries = if text.lines().any(|l| l.trim().starts_with("#EXT-X-")) {
                    match hls::parse(&text, &base)? {
                        hls::Playlist::Master(entries) => entries,
                        hls::Playlist::Media(media) => {
                            return Ok(Some(Session::Hls(Box::new(hls::Session::new(
                                base, media,
                            )))));
                        }
                    }
                } else {
                    playlist(&text, pls, &base)?
                };
                if entries.is_empty() || entries.len() > 64 {
                    return Err("playlist must contain 1..64 stream alternatives".into());
                }
                let count = entries.len();
                entries.rotate_left(self.rotation % count);
                self.pending
                    .extend(entries.into_iter().rev().map(|u| (u, depth + 1)));
                Ok(None)
            })();
            match attempt {
                Ok(Some(session)) => return Ok(Some(session)),
                Ok(None) => (),
                Err(e) => self.last_error = Some(e.to_string()),
            }
        }
        match self.last_error.take() {
            Some(e) => Err(e.into()),
            None => Ok(None),
        }
    }
}
fn playlist(text: &str, pls: bool, base: &url::Url) -> Result<Vec<url::Url>> {
    let text = text.trim_start_matches('\u{feff}');
    let mut entries = Vec::new();
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if pls {
            if let Some((key, value)) = line.split_once('=') {
                let key = key.trim().to_ascii_lowercase();
                if let Some(index) = key
                    .strip_prefix("file")
                    .and_then(|n| n.parse::<usize>().ok())
                {
                    entries.push((index, value.trim()));
                }
            }
        } else if !line.starts_with('#') {
            entries.push((entries.len(), line));
        }
    }
    entries.sort_by_key(|e| e.0);
    let urls: Vec<_> = entries
        .into_iter()
        .filter_map(|(_, entry)| {
            if entry.is_empty() || entry.chars().any(|c| c.is_whitespace() || c.is_control()) {
                return None;
            }
            let url = base.join(entry).ok()?;
            validate_url(url.as_str()).ok()
        })
        .collect();
    if urls.is_empty() {
        return Err("playlist contains no HTTP(S) stream entries".into());
    }
    Ok(urls)
}

pub struct IcyReader<R> {
    inner: R,
    interval: Option<usize>,
    remaining: usize,
    encoding: Option<&'static encoding_rs::Encoding>,
    status: SharedStatus,
}
impl<R: Read> IcyReader<R> {
    pub fn with_encoding(mut self, encoding: Option<&'static encoding_rs::Encoding>) -> Self {
        self.encoding = encoding;
        self
    }
    pub fn new(inner: R, interval: Option<usize>, status: SharedStatus) -> Result<Self> {
        if interval == Some(0) {
            return Err("icy-metaint must be positive".into());
        }
        Ok(Self {
            inner,
            interval,
            remaining: interval.unwrap_or(0),
            encoding: None,
            status,
        })
    }
}
impl<R: std::io::Seek> std::io::Seek for IcyReader<R> {
    fn seek(&mut self, position: std::io::SeekFrom) -> io::Result<u64> {
        if self.interval.is_some() {
            return Err(io::ErrorKind::Unsupported.into());
        }
        self.inner.seek(position)
    }
}
impl<R: Read> Read for IcyReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let Some(interval) = self.interval else {
            return self.inner.read(buf);
        };
        if self.remaining == 0 {
            let mut length = [0];
            self.inner.read_exact(&mut length)?;
            // ICY length is one byte in units of 16 bytes, at most 4080 bytes.
            let mut metadata = [0; 4080];
            let size = usize::from(length[0]) * 16;
            self.inner.read_exact(&mut metadata[..size])?;
            if size > 0 {
                let bytes = &metadata[..size];
                let encoding = self.encoding.unwrap_or_else(|| {
                    if std::str::from_utf8(bytes).is_ok() {
                        encoding_rs::UTF_8
                    } else {
                        encoding_rs::WINDOWS_1252
                    }
                });
                let (text, _, _) = encoding.decode(bytes);
                if let Some(rest) = text.split_once("StreamTitle='").map(|(_, r)| r)
                    && let Some((title, _)) = rest.split_once("';")
                    && let Ok(mut state) = self.status.lock()
                {
                    state.title = title
                        .chars()
                        .filter(|c| !c.is_control())
                        .take(512)
                        .collect();
                }
            }
            self.remaining = interval;
        }
        let limit = buf.len().min(self.remaining);
        let count = self.inner.read(&mut buf[..limit])?;
        self.remaining -= count;
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    #[test]
    fn playlists_resolve_relative_entries() {
        let base = url::Url::parse("https://example.org/radio/list.pls").unwrap();
        assert_eq!(
            playlist(
                "[playlist]\nFile2=https://example.org/two\nFile1=../one",
                true,
                &base
            )
            .unwrap()[0]
                .as_str(),
            "https://example.org/one"
        );
        assert_eq!(
            playlist("#EXTM3U\n#EXTINF:-1,Station\nstream", false, &base).unwrap()[0].as_str(),
            "https://example.org/radio/stream"
        );
        assert!(playlist("file:///tmp/audio", false, &base).is_err());
    }
    #[test]
    fn icy_strips_blocks_with_short_reads_and_sanitizes_title() {
        let status = SharedStatus::default();
        let mut data = b"abc\x02".to_vec();
        let mut meta = b"StreamTitle='hi\x1b';StreamUrl='x';".to_vec();
        meta.resize(32, 0);
        data.extend(meta);
        data.extend(b"def\x00ghi");
        let mut reader = IcyReader::new(&data[..], Some(3), status.clone()).unwrap();
        let mut out = Vec::new();
        for _ in 0..9 {
            let mut b = [0];
            reader.read_exact(&mut b).unwrap();
            out.push(b[0]);
        }
        assert_eq!(out, b"abcdefghi");
        assert_eq!(status.lock().unwrap().title, "hi");
        assert!(reader.read(&mut [0]).is_err()); // missing length byte
        assert!(IcyReader::new(&b""[..], Some(0), status).is_err());
    }
    #[test]
    fn idle_timeout_expires_on_a_stalled_body() {
        use std::{io::Write, net::TcpListener, thread, time::Instant};
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let _ = s.read(&mut [0; 4096]);
            s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\nx")
                .unwrap();
            thread::sleep(Duration::from_millis(700));
        });
        let start = Instant::now();
        let mut response = agent(Duration::from_millis(100))
            .get(format!("http://{addr}"))
            .call()
            .unwrap();
        assert!(response.body_mut().read_to_vec().is_err());
        assert!(start.elapsed() < Duration::from_millis(650));
        server.join().unwrap();
    }
    #[test]
    fn healthy_body_can_outlive_idle_timeout() {
        use std::{io::Write, net::TcpListener, thread};
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let _ = stream.read(&mut [0; 4096]);
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\n")
                .unwrap();
            for _ in 0..5 {
                stream.write_all(b"x").unwrap();
                thread::sleep(Duration::from_millis(100));
            }
        });
        let mut response = agent(Duration::from_millis(300))
            .get(format!("http://{addr}"))
            .call()
            .unwrap();
        assert_eq!(response.body_mut().read_to_vec().unwrap(), b"xxxxx");
        server.join().unwrap();
    }
    #[test]
    fn decodes_legacy_metadata_and_honors_explicit_encoding() {
        for (title, encoding, expected) in [
            (b"Caf\xe9".as_slice(), None, "Café"),
            ("音楽".as_bytes(), None, "音楽"),
            (
                b"\xcf\xf0\xe8\xe2\xe5\xf2".as_slice(),
                Some(encoding_rs::WINDOWS_1251),
                "Привет",
            ),
        ] {
            let mut metadata = b"StreamTitle='".to_vec();
            metadata.extend(title);
            metadata.extend(b"';");
            let blocks = metadata.len().div_ceil(16);
            metadata.resize(blocks * 16, 0);
            let mut data = vec![b'a', blocks as u8];
            data.extend(metadata);
            data.push(b'b');
            let status = SharedStatus::default();
            let mut reader = IcyReader::new(&data[..], Some(1), status.clone())
                .unwrap()
                .with_encoding(encoding);
            let mut audio = [0; 2];
            reader.read_exact(&mut audio).unwrap();
            assert_eq!(&audio, b"ab");
            assert_eq!(status.lock().unwrap().title, expected);
        }
    }
}
