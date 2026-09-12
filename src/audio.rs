//! HTTP and decoding run off the audio callback. A bounded PCM queue limits memory.
use crate::{
    Result,
    radio::{self, IcyReader, SharedStatus},
};
use rodio::{ChannelCount, Decoder, SampleRate, Source};
use std::{
    io::{self, Read, Seek, SeekFrom},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, TryRecvError},
    },
    thread,
    time::Duration,
};

mod decoded;

type Failure = Arc<Mutex<Option<String>>>;

struct StreamReader<R> {
    inner: R,
    failure: Failure,
}
impl<R: Read> Read for StreamReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf).inspect_err(|e| {
            if let Ok(mut failure) = self.failure.lock() {
                *failure = Some(format!("stream read failed: {e}"));
            }
        })
    }
}
impl<R: Seek> Seek for StreamReader<R> {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.inner.seek(position)
    }
}

pub struct Audio {
    rx: Receiver<Vec<f32>>,
    pending: std::vec::IntoIter<f32>,
    silence_left: usize,
    refill: Option<Vec<f32>>,
    buffering: bool,
    channels: ChannelCount,
    rate: SampleRate,
    pub failure: Failure,
    pub status: SharedStatus,
    alive: Arc<AtomicBool>,
}
impl Drop for Audio {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::Relaxed);
    }
}
impl Iterator for Audio {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        if let Some(sample) = self.pending.next() {
            return Some(sample);
        }
        if self.silence_left > 0 {
            self.silence_left -= 1;
            return Some(0.0);
        }
        if !self.buffering
            && let Some(chunk) = self.refill.take()
        {
            self.pending = chunk.into_iter();
            return self.pending.next();
        }
        match self.rx.try_recv() {
            Ok(chunk) => {
                if self.buffering {
                    if let Some(first) = self.refill.replace(chunk) {
                        self.pending = first.into_iter();
                        self.buffering = false;
                        self.pending.next()
                    } else {
                        // Wait for a second block before resuming after starvation.
                        self.silence_left = self.channels.get() as usize - 1;
                        Some(0.0)
                    }
                } else {
                    self.pending = chunk.into_iter();
                    self.pending.next()
                }
            }
            // Keep the device callback nonblocking during a network stall.
            Err(TryRecvError::Empty) => {
                self.buffering = true;
                self.silence_left = self.channels.get() as usize - 1;
                Some(0.0)
            }
            Err(TryRecvError::Disconnected) => {
                self.pending = self.refill.take()?.into_iter();
                self.buffering = false;
                self.pending.next()
            }
        }
    }
}
impl Source for Audio {
    fn current_span_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> ChannelCount {
        self.channels
    }
    fn sample_rate(&self) -> SampleRate {
        self.rate
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}
impl Audio {
    pub fn check(self, stopped: &AtomicBool) -> Result<()> {
        let target = self.rate.get() as usize * self.channels.get() as usize;
        let mut count = 0;
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while count < target && !stopped.load(Ordering::Relaxed) {
            match self.rx.recv_timeout(Duration::from_millis(100)) {
                Ok(chunk) => count += chunk.len(),
                Err(RecvTimeoutError::Timeout) => {
                    if std::time::Instant::now() >= deadline {
                        return Err("timed out waiting for decoded audio".into());
                    }
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => {
                    if stopped.load(Ordering::Relaxed) {
                        return Ok(());
                    }
                    return Err(format!(
                        "stream ended after {count}/{target} samples: {}",
                        self.failure
                            .lock()
                            .map_err(|_| "worker state unavailable")?
                            .as_deref()
                            .unwrap_or("end of stream")
                    )
                    .into());
                }
            }
        }
        if count >= target {
            println!(
                "decoded {count} samples: {} Hz, {} channel(s)",
                self.rate, self.channels
            );
        }
        Ok(())
    }
}

#[cfg(test)]
pub fn connect(url: &str, stopped: &Arc<AtomicBool>, reconnect: bool) -> Result<Option<Audio>> {
    connect_with_idle(url, stopped, reconnect, Duration::from_secs(10), None)
}

pub fn connect_encoded(
    url: &str,
    stopped: &Arc<AtomicBool>,
    reconnect: bool,
    encoding: Option<&'static encoding_rs::Encoding>,
) -> Result<Option<Audio>> {
    connect_with_idle(url, stopped, reconnect, Duration::from_secs(10), encoding)
}

fn connect_with_idle(
    url: &str,
    stopped: &Arc<AtomicBool>,
    reconnect: bool,
    idle: Duration,
    encoding: Option<&'static encoding_rs::Encoding>,
) -> Result<Option<Audio>> {
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let (tx, rx) = mpsc::sync_channel(8);
    let failure: Failure = Arc::new(Mutex::new(None));
    let status = SharedStatus::default();
    let worker_status = status.clone();
    let worker_failure = failure.clone();
    let cancelled = stopped.clone();
    let alive = Arc::new(AtomicBool::new(true));
    let worker_alive = alive.clone();
    let url = url.to_string();
    thread::spawn(move || {
        let agent = radio::agent(idle);
        let active = || !cancelled.load(Ordering::Relaxed) && worker_alive.load(Ordering::Relaxed);
        let mut format = None;
        let mut retries = 0;
        let mut rotation = 0;
        let mut checkpoint = None;
        while active() {
            let mut produced = 0;

            if let Ok(mut state) = worker_status.lock() {
                state.message = "connecting".into();
                state.title.clear();
            }
            if let Ok(mut failure) = worker_failure.lock() {
                *failure = None;
            }
            let mut run = || -> Result<()> {
                let mut resolver = radio::Resolver::new(&url, rotation)?;
                let mut last_error = "no playable stream entries".to_string();
                while let Some(mut session) = resolver.next(&agent, &active)? {
                    session.resume(checkpoint.as_ref());
                    let mut receive = || -> Result<()> {
                        while let Some(input) = session.next(&agent, &active)? {
                            let before = produced;
                            if let Ok(mut failure) = worker_failure.lock() {
                                *failure = None;
                            }
                            let server_encoding = input
                                .mime
                                .split(';')
                                .find_map(|part| part.trim().strip_prefix("charset="))
                                .and_then(|s| {
                                    encoding_rs::Encoding::for_label(s.trim_matches('"').as_bytes())
                                });
                            let reader = StreamReader {
                                inner: IcyReader::new(
                                    input.data,
                                    input.interval,
                                    worker_status.clone(),
                                )?
                                .with_encoding(encoding.or(server_encoding)),
                                failure: worker_failure.clone(),
                            };
                            let mut builder = Decoder::builder()
                                .with_data(reader)
                                .with_mime_type(&input.mime);
                            if let Some(length) = input.byte_len {
                                builder = builder.with_byte_len(length);
                            }
                            let decoder = builder.build()?;
                            let decoder = decoded::PrimedDecoder::new(decoder);
                            let channels = decoder.channels();
                            let rate = decoder.sample_rate();
                            if format.is_none() {
                                format = Some((channels, rate));
                                ready_tx.send((channels, rate))?;
                            }
                            let (channels, rate) = format.unwrap();
                            // Keep all conversion off the device callback, including span changes.
                            let mut decoder =
                                rodio::source::UniformSourceIterator::new(decoder, channels, rate);
                            if let Ok(mut state) = worker_status.lock() {
                                state.message = "buffering".into();
                            }
                            // Prime each connection with two blocks before exposing its samples.
                            let mut primed = Vec::with_capacity(2);
                            let mut buffering = true;
                            loop {
                                if !active() {
                                    return Ok(());
                                }
                                let chunk: Vec<_> = decoder
                                    .by_ref()
                                    .take(2048 * channels.get() as usize)
                                    .collect();
                                let ended = chunk.is_empty();
                                if ended && primed.is_empty() {
                                    if input.finite {
                                        break;
                                    }
                                    return Err("stream ended".into());
                                }
                                if chunk.len() % channels.get() as usize != 0 {
                                    return Err("incomplete audio frame".into());
                                }
                                produced += chunk.len();
                                if !ended {
                                    primed.push(chunk);
                                }
                                if !ended && buffering && primed.len() < 2 {
                                    continue;
                                }
                                buffering = false;
                                for mut chunk in primed.drain(..) {
                                    loop {
                                        if !active() {
                                            return Ok(());
                                        }
                                        match tx.try_send(chunk) {
                                            Ok(()) => break,
                                            Err(mpsc::TrySendError::Full(value)) => {
                                                chunk = value;
                                                thread::sleep(Duration::from_millis(10));
                                            }
                                            Err(mpsc::TrySendError::Disconnected(_)) => {
                                                return Ok(());
                                            }
                                        }
                                    }
                                }
                                if ended {
                                    if input.finite {
                                        break;
                                    }
                                    return Err("stream ended".into());
                                }
                                if let Ok(mut state) = worker_status.lock() {
                                    state.message = "playing".into();
                                }
                            }

                            if let Some(error) = worker_failure
                                .lock()
                                .map_err(|_| "worker state unavailable")?
                                .take()
                            {
                                return Err(error.into());
                            }
                            if produced == before {
                                return Err("segment decoded no audio".into());
                            }
                        }
                        if format.is_none() && active() {
                            return Err("stream ended without audio".into());
                        }
                        Ok(())
                    };
                    match receive() {
                        Ok(()) => return Ok(()),
                        Err(e) => {
                            last_error = e.to_string();
                            if let Some(position) = session.checkpoint() {
                                checkpoint = Some(position);
                            }
                        }
                    }
                    if !active() {
                        return Ok(());
                    }
                }
                Err(last_error.into())
            };
            let error = match run() {
                Ok(()) => break,
                Err(error) => error.to_string(),
            };
            if !active() {
                break;
            }
            if !reconnect {
                if let Ok(mut failure) = worker_failure.lock() {
                    *failure = Some(error);
                }
                break;
            }
            if format.is_some_and(|(channels, rate)| {
                produced >= channels.get() as usize * rate.get() as usize
            }) {
                retries = 0;
            }
            if retries == 5 {
                if let Ok(mut failure) = worker_failure.lock() {
                    *failure = Some(format!("reconnect exhausted: {error}"));
                }
                break;
            }
            rotation = rotation.wrapping_add(1);
            let delay = 1 << retries;
            retries += 1;
            if let Ok(mut state) = worker_status.lock() {
                state.message = format!("reconnecting in {delay}s ({retries}/5)");
            }
            for _ in 0..delay * 10 {
                if !active() {
                    break;
                }
                thread::sleep(Duration::from_millis(100));
            }
        }
    });
    loop {
        if stopped.load(Ordering::Relaxed) {
            alive.store(false, Ordering::Relaxed);
            return Ok(None);
        }
        let ready = ready_rx.recv_timeout(Duration::from_millis(100));
        // Cancellation may close the channel while recv_timeout is waiting.
        // Recheck before treating disconnection as a failure or opening a device.
        if stopped.load(Ordering::Relaxed) {
            alive.store(false, Ordering::Relaxed);
            return Ok(None);
        }
        match ready {
            Ok((channels, rate)) => {
                return Ok(Some(Audio {
                    rx,
                    pending: Vec::new().into_iter(),
                    silence_left: 0,
                    refill: None,
                    buffering: false,
                    channels,
                    rate,
                    failure,
                    status,
                    alive,
                }));
            }
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => {
                return Err(failure
                    .lock()
                    .map_err(|_| "audio worker failed")?
                    .take()
                    .unwrap_or_else(|| "audio worker stopped".into())
                    .into());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn starvation_keeps_stereo_frames_aligned_and_end_drains() {
        let (tx, rx) = mpsc::sync_channel(2);
        let mut audio = Audio {
            rx,
            pending: vec![1.0, 2.0].into_iter(),
            silence_left: 0,
            refill: None,
            buffering: false,
            channels: ChannelCount::new(2).unwrap(),
            rate: SampleRate::new(8000).unwrap(),
            failure: Arc::new(Mutex::new(None)),
            status: SharedStatus::default(),
            alive: Arc::new(AtomicBool::new(true)),
        };
        assert_eq!(audio.next(), Some(1.0));
        assert_eq!(audio.next(), Some(2.0));
        assert_eq!(audio.next(), Some(0.0));
        tx.send(vec![3.0, 4.0]).unwrap();
        // A refill between left and right must not shift channel assignment.
        assert_eq!(audio.next(), Some(0.0));
        drop(tx);
        assert_eq!(audio.collect::<Vec<_>>(), vec![0.0, 0.0, 3.0, 4.0]);
    }
    fn wav(rate: u32) -> Vec<u8> {
        wav_channels(rate, 1)
    }
    fn wav_channels(rate: u32, channels: u16) -> Vec<u8> {
        let samples = 16000u32 * u32::from(channels);
        let mut out = Vec::new();
        out.extend(b"RIFF");
        out.extend((36 + samples * 2).to_le_bytes());
        out.extend(b"WAVEfmt ");
        out.extend(16u32.to_le_bytes());
        out.extend(1u16.to_le_bytes());
        out.extend(channels.to_le_bytes());
        out.extend(rate.to_le_bytes());
        out.extend((rate * 2 * u32::from(channels)).to_le_bytes());
        out.extend((2 * channels).to_le_bytes());
        out.extend(16u16.to_le_bytes());
        out.extend(b"data");
        out.extend((samples * 2).to_le_bytes());
        out.resize(out.len() + samples as usize * 2, 1);
        out
    }
    #[test]
    fn reconnects_after_eof_or_stall_and_normalizes_format_changes() {
        use std::{io::Write, net::TcpListener, time::Instant};
        for (stall, changed) in [(false, false), (true, false), (false, true)] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            listener.set_nonblocking(true).unwrap();
            let server = thread::spawn(move || {
                let deadline = Instant::now() + Duration::from_secs(8);
                for attempt in 0..2 {
                    let mut stream = loop {
                        match listener.accept() {
                            Ok((s, _)) => break s,
                            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                                assert!(Instant::now() < deadline, "missing reconnect");
                                thread::sleep(Duration::from_millis(10));
                            }
                            Err(e) => panic!("{e}"),
                        }
                    };
                    stream.set_nonblocking(false).unwrap();
                    stream
                        .set_read_timeout(Some(Duration::from_secs(2)))
                        .unwrap();
                    let _ = stream.read(&mut [0; 4096]);
                    let mut body = if changed && attempt == 1 {
                        wav_channels(16000, 2)
                    } else {
                        wav(8000)
                    };
                    if stall && attempt == 0 {
                        body[4..8].copy_from_slice(&64036u32.to_le_bytes());
                        body[40..44].copy_from_slice(&64000u32.to_le_bytes());
                    }
                    write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: audio/wav\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len() + if stall && attempt == 0 { 100 } else { 0 }).unwrap();
                    stream.write_all(&body).unwrap();
                    if stall && attempt == 0 {
                        thread::sleep(Duration::from_millis(600));
                    }
                }
            });
            let stopped = Arc::new(AtomicBool::new(false));
            let audio = connect_with_idle(
                &format!("http://{addr}/radio"),
                &stopped,
                true,
                Duration::from_millis(150),
                None,
            )
            .unwrap()
            .unwrap();
            let deadline = Instant::now() + Duration::from_secs(6);
            let mut samples = 0;
            loop {
                match audio.rx.recv_timeout(Duration::from_millis(100)) {
                    Ok(chunk) => {
                        samples += chunk.len();
                        if samples > 16000 {
                            break;
                        }
                    }
                    Err(RecvTimeoutError::Disconnected) => break,
                    Err(RecvTimeoutError::Timeout) => assert!(Instant::now() < deadline),
                }
            }
            assert!(samples > 16000);
            assert_eq!(audio.rate.get(), 8000);
            assert_eq!(audio.channels.get(), 1);
            drop(audio);
            server.join().unwrap();
        }
    }
    #[test]
    fn dropping_receiver_signals_cancellation_and_disconnects_queue() {
        // A full queue is handled with cancellable try_send, never a blocking send.
        let (tx, rx) = mpsc::sync_channel(1);
        tx.send(vec![1.0]).unwrap();
        assert!(matches!(
            tx.try_send(vec![2.0]),
            Err(mpsc::TrySendError::Full(_))
        ));
        let alive = Arc::new(AtomicBool::new(true));
        let audio = Audio {
            rx,
            pending: Vec::new().into_iter(),
            silence_left: 0,
            refill: None,
            buffering: false,
            channels: ChannelCount::new(1).unwrap(),
            rate: SampleRate::new(8000).unwrap(),
            failure: Failure::default(),
            status: SharedStatus::default(),
            alive: alive.clone(),
        };
        drop(audio);
        assert!(!alive.load(Ordering::Relaxed));
        assert!(matches!(
            tx.try_send(vec![2.0]),
            Err(mpsc::TrySendError::Disconnected(_))
        ));
    }
    #[test]
    fn cancels_connection_setup_during_retry_backoff() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            use std::io::Write;
            let (mut stream, _) = listener.accept().unwrap();
            let _ = stream.read(&mut [0; 4096]);
            stream
                .write_all(
                    b"HTTP/1.1 503 Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .unwrap();
        });
        let stopped = Arc::new(AtomicBool::new(false));
        let signal = stopped.clone();
        let stop = thread::spawn(move || {
            thread::sleep(Duration::from_millis(200));
            signal.store(true, Ordering::Relaxed);
        });
        let start = std::time::Instant::now();
        assert!(
            connect(&format!("http://{address}"), &stopped, true)
                .unwrap()
                .is_none()
        );
        assert!(start.elapsed() < Duration::from_secs(2));
        stop.join().unwrap();
        server.join().unwrap();
    }
    #[test]
    fn hls_finishes_vod_and_advances_live_windows_without_replaying_segments() {
        use crate::test_support::{Response, Server};
        for live in [false, true] {
            let server = Server::new(move |path, _, count| {
                let body: Vec<u8> = if path == "/radio.m3u8" {
                    if live {
                        let text = if count == 1 {
                            "#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXT-X-MEDIA-SEQUENCE:10\n#EXTINF:1,\nseg10.aac\n#EXTINF:1,\nseg11.aac\n"
                        } else {
                            "#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXT-X-MEDIA-SEQUENCE:11\n#EXTINF:1,\nseg11.aac\n#EXTINF:1,\nseg12.aac\n#EXT-X-ENDLIST\n"
                        };
                        text.as_bytes().to_vec()
                    } else {
                        include_bytes!("../tests/fixtures/media.m3u8").to_vec()
                    }
                } else if live {
                    include_bytes!("../tests/fixtures/tone.aac").to_vec()
                } else {
                    match path {
                        "/init.mp4" => include_bytes!("../tests/fixtures/init.mp4").to_vec(),
                        "/part00.m4s" => include_bytes!("../tests/fixtures/part00.m4s").to_vec(),
                        "/part01.m4s" => include_bytes!("../tests/fixtures/part01.m4s").to_vec(),
                        "/part02.m4s" => include_bytes!("../tests/fixtures/part02.m4s").to_vec(),
                        _ => panic!("unexpected {path}"),
                    }
                };
                Response::new(
                    body,
                    if path.ends_with("m3u8") {
                        "application/vnd.apple.mpegurl"
                    } else {
                        "application/octet-stream"
                    },
                )
            });
            let stopped = Arc::new(AtomicBool::new(false));
            let audio = connect(&format!("{}/radio.m3u8", server.url), &stopped, false)
                .unwrap()
                .unwrap();
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            let mut samples = 0;
            loop {
                match audio.rx.recv_timeout(Duration::from_millis(100)) {
                    Ok(chunk) => samples += chunk.len(),
                    Err(RecvTimeoutError::Disconnected) => break,
                    Err(RecvTimeoutError::Timeout) => assert!(std::time::Instant::now() < deadline),
                }
            }
            assert!(
                audio.failure.lock().unwrap().is_none(),
                "{:?}",
                audio.failure.lock().unwrap()
            );
            assert_eq!(audio.rate.get(), 16000);
            if live {
                assert!((3 * 32000..3 * 36000).contains(&samples), "{samples}");
                let requests = server.requests.lock().unwrap();
                for path in ["/seg10.aac", "/seg11.aac", "/seg12.aac"] {
                    assert_eq!(requests.iter().filter(|p| p.as_str() == path).count(), 1);
                }
            } else {
                assert!((32000..37000).contains(&samples), "{samples}");
            }
        }
    }
    #[test]
    fn hls_retries_failed_segment_without_replaying_completed_segment() {
        use crate::test_support::{Response, Server};
        let server = Server::new(|path, _, count| {
            if path == "/radio.m3u8" {
                Response::new(
                    "#EXTM3U\n#EXT-X-TARGETDURATION:3\n#EXT-X-MEDIA-SEQUENCE:10\n#EXTINF:2.1,\nfirst.aac\n#EXTINF:2.1,\nsecond.aac\n#EXT-X-ENDLIST",
                    "application/vnd.apple.mpegurl",
                )
            } else if path == "/second.aac" && count == 1 {
                let mut response = Response::new("temporary failure", "text/plain");
                response.status = "HTTP/1.1 503 Unavailable";
                response
            } else {
                Response::new(
                    include_bytes!("../tests/fixtures/tone.aac").as_slice(),
                    "audio/aac",
                )
            }
        });
        let stopped = Arc::new(AtomicBool::new(false));
        let audio = connect(&format!("{}/radio.m3u8", server.url), &stopped, true)
            .unwrap()
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let mut samples = 0;
        loop {
            match audio.rx.recv_timeout(Duration::from_millis(100)) {
                Ok(chunk) => samples += chunk.len(),
                Err(RecvTimeoutError::Disconnected) => break,
                Err(RecvTimeoutError::Timeout) => assert!(std::time::Instant::now() < deadline),
            }
        }
        let error = audio.failure.lock().unwrap().clone();
        assert!(error.is_none(), "{error:?}");
        assert!((64000..72000).contains(&samples), "{samples}");
        let requests = server.requests.lock().unwrap();
        assert_eq!(
            requests
                .iter()
                .filter(|p| p.as_str() == "/first.aac")
                .count(),
            1
        );
        assert_eq!(
            requests
                .iter()
                .filter(|p| p.as_str() == "/second.aac")
                .count(),
            2
        );
    }
}
