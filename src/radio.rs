//! Bounded playlist and ICY parsing; all work happens on the decoder thread.
use crate::{Result, config::validate_url};
use std::{
    io::{self, Read},
    sync::{Arc, Mutex},
    time::Duration,
};
use ureq::{
    ResponseExt,
    unversioned::{
        resolver::DefaultResolver,
        transport::{
            Buffers, ConnectionDetails, Connector, DefaultConnector, NextTimeout, Transport,
        },
    },
};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Status {
    pub message: String,
    pub title: String,
}
pub type SharedStatus = Arc<Mutex<Status>>;

// ureq's body timeout is a total budget, unsuitable for endless radio. Keep
// its normal connector (TLS/proxy support), but cap each blocking input wait.
// This unversioned interface is why the ureq dependency is pinned exactly.
#[derive(Debug)]
struct IdleConnector(Duration);
#[derive(Debug)]
struct IdleTransport<T>(T, Duration);
impl<T: Transport> Connector<T> for IdleConnector {
    type Out = IdleTransport<T>;
    fn connect(
        &self,
        _: &ConnectionDetails,
        transport: Option<T>,
    ) -> std::result::Result<Option<Self::Out>, ureq::Error> {
        Ok(transport.map(|t| IdleTransport(t, self.0)))
    }
}
impl<T: Transport> Transport for IdleTransport<T> {
    fn buffers(&mut self) -> &mut dyn Buffers {
        self.0.buffers()
    }
    fn transmit_output(
        &mut self,
        amount: usize,
        timeout: NextTimeout,
    ) -> std::result::Result<(), ureq::Error> {
        self.0.transmit_output(amount, timeout)
    }
    fn await_input(&mut self, mut timeout: NextTimeout) -> std::result::Result<bool, ureq::Error> {
        if timeout.after > self.1.into() {
            timeout.after = self.1.into();
            timeout.reason = ureq::Timeout::RecvBody;
        }
        self.0.await_input(timeout)
    }
    fn is_open(&mut self) -> bool {
        self.0.is_open()
    }
    fn is_tls(&self) -> bool {
        self.0.is_tls()
    }
}
pub fn agent(idle: Duration) -> ureq::Agent {
    let config = ureq::Agent::config_builder()
        .timeout_resolve(Some(Duration::from_secs(10)))
        .timeout_connect(Some(Duration::from_secs(10)))
        .timeout_send_request(Some(Duration::from_secs(10)))
        .timeout_recv_response(Some(Duration::from_secs(15)))
        .max_redirects(5)
        .build();
    ureq::Agent::with_parts(
        config,
        DefaultConnector::default().chain(IdleConnector(idle)),
        DefaultResolver::default(),
    )
}
const MAX_PLAYLIST: u64 = 64 * 1024;

pub fn open(agent: &ureq::Agent, input: &str) -> Result<ureq::http::Response<ureq::Body>> {
    let mut url = validate_url(input)?;
    let mut visited = std::collections::HashSet::new();
    for _ in 0..5 {
        if !visited.insert(url.to_string()) {
            return Err("playlist cycle detected".into());
        }
        let response = agent.get(url.as_str()).header("Icy-MetaData", "1").call()?;
        let base = validate_url(&response.get_uri().to_string())?;
        let mime = response
            .headers()
            .get("content-type")
            .and_then(|h| h.to_str().ok())
            .unwrap_or("")
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        let path = base.path().to_ascii_lowercase();
        let pls = mime == "audio/x-scpls" || path.ends_with(".pls");
        let m3u = matches!(
            mime.as_str(),
            "audio/x-mpegurl"
                | "audio/mpegurl"
                | "application/x-mpegurl"
                | "application/vnd.apple.mpegurl"
        ) || path.ends_with(".m3u")
            || path.ends_with(".m3u8");
        if !pls && !m3u {
            return Ok(response);
        }
        let mut bytes = Vec::new();
        response
            .into_body()
            .into_reader()
            .take(MAX_PLAYLIST + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_PLAYLIST {
            return Err("playlist exceeds 64 KiB".into());
        }
        let text = std::str::from_utf8(&bytes).map_err(|_| "playlist must be UTF-8")?;
        url = playlist(text, pls, &base)?;
    }
    Err("playlist nesting exceeds five requests".into())
}
fn playlist(text: &str, pls: bool, base: &url::Url) -> Result<url::Url> {
    let text = text.trim_start_matches('\u{feff}');
    if text.lines().any(|line| line.trim().starts_with("#EXT-X-")) {
        return Err("HLS playlists are not supported".into());
    }
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
    for (_, entry) in entries {
        if entry.chars().any(|c| c.is_whitespace() || c.is_control()) {
            continue;
        }
        if let Ok(url) = base.join(entry)
            && validate_url(url.as_str()).is_ok()
        {
            return Ok(url);
        }
    }
    Err("playlist contains no HTTP(S) stream entries".into())
}

pub struct IcyReader<R> {
    inner: R,
    interval: Option<usize>,
    remaining: usize,
    status: SharedStatus,
}
impl<R: Read> IcyReader<R> {
    pub fn new(inner: R, interval: Option<usize>, status: SharedStatus) -> Result<Self> {
        if interval == Some(0) {
            return Err("icy-metaint must be positive".into());
        }
        Ok(Self {
            inner,
            interval,
            remaining: interval.unwrap_or(0),
            status,
        })
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
                let text = String::from_utf8_lossy(&metadata[..size]);
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
    #[test]
    fn playlists_resolve_relative_entries_and_reject_hls() {
        let base = url::Url::parse("https://example.org/radio/list.pls").unwrap();
        assert_eq!(
            playlist(
                "[playlist]\nFile2=https://example.org/two\nFile1=../one",
                true,
                &base
            )
            .unwrap()
            .as_str(),
            "https://example.org/one"
        );
        assert_eq!(
            playlist("#EXTM3U\n#EXTINF:-1,Station\nstream", false, &base)
                .unwrap()
                .as_str(),
            "https://example.org/radio/stream"
        );
        assert!(
            playlist(
                "#EXTM3U\n#EXT-X-TARGETDURATION:10\nsegment.aac",
                false,
                &base
            )
            .is_err()
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
}
