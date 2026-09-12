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
impl<R> Seek for StreamReader<R> {
    fn seek(&mut self, _: SeekFrom) -> io::Result<u64> {
        Err(io::ErrorKind::Unsupported.into())
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
                    return Err("stream ended before one second could be decoded".into());
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

pub fn connect(url: &str, stopped: &Arc<AtomicBool>, reconnect: bool) -> Result<Option<Audio>> {
    connect_with_idle(url, stopped, reconnect, Duration::from_secs(10))
}

fn connect_with_idle(
    url: &str,
    stopped: &Arc<AtomicBool>,
    reconnect: bool,
    idle: Duration,
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
        while active() {
            let mut produced = 0;
            let mut format_changed = false;
            if let Ok(mut state) = worker_status.lock() {
                state.message = "connecting".into();
                state.title.clear();
            }
            if let Ok(mut failure) = worker_failure.lock() {
                *failure = None;
            }
            let mut run = || -> Result<()> {
                let response = radio::open(&agent, &url)?;
                let interval = response
                    .headers()
                    .get("icy-metaint")
                    .map(|h| -> Result<usize> { Ok(h.to_str()?.parse()?) })
                    .transpose()?;
                let mime = response
                    .headers()
                    .get("content-type")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("")
                    .to_string();
                let reader = StreamReader {
                    inner: IcyReader::new(
                        response.into_body().into_reader(),
                        interval,
                        worker_status.clone(),
                    )?,
                    failure: worker_failure.clone(),
                };
                let mut decoder = Decoder::builder()
                    .with_data(reader)
                    .with_seekable(false)
                    .with_mime_type(&mime)
                    .build()?;
                let channels = decoder.channels();
                let rate = decoder.sample_rate();
                if format.is_some_and(|f| f != (channels, rate)) {
                    format_changed = true;
                    return Err("stream audio format changed; restart rxer".into());
                }
                if format.is_none() {
                    format = Some((channels, rate));
                    ready_tx.send((channels, rate))?;
                }
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
                    if decoder.channels() != channels || decoder.sample_rate() != rate {
                        format_changed = true;
                        return Err("stream audio format changed; restart rxer".into());
                    }
                    let ended = chunk.is_empty();
                    if ended && primed.is_empty() {
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
                                Err(mpsc::TrySendError::Disconnected(_)) => return Ok(()),
                            }
                        }
                    }
                    if ended {
                        return Err("stream ended".into());
                    }
                    if let Ok(mut state) = worker_status.lock() {
                        state.message = "playing".into();
                    }
                }
            };
            let error = match run() {
                Ok(()) => break,
                Err(error) => error.to_string(),
            };
            if !active() {
                break;
            }
            if !reconnect || format_changed {
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
        let samples = 16000u32;
        let mut out = Vec::new();
        out.extend(b"RIFF");
        out.extend((36 + samples * 2).to_le_bytes());
        out.extend(b"WAVEfmt ");
        out.extend(16u32.to_le_bytes());
        out.extend(1u16.to_le_bytes());
        out.extend(1u16.to_le_bytes());
        out.extend(rate.to_le_bytes());
        out.extend((rate * 2).to_le_bytes());
        out.extend(2u16.to_le_bytes());
        out.extend(16u16.to_le_bytes());
        out.extend(b"data");
        out.extend((samples * 2).to_le_bytes());
        out.resize(out.len() + samples as usize * 2, 1);
        out
    }
    #[test]
    fn reconnects_after_eof_or_stall_and_rejects_format_changes() {
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
                    stream
                        .set_read_timeout(Some(Duration::from_secs(2)))
                        .unwrap();
                    let _ = stream.read(&mut [0; 4096]);
                    let mut body = wav(if changed && attempt == 1 { 16000 } else { 8000 });
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
            if changed {
                assert_eq!(samples, 16000);
                assert!(
                    audio
                        .failure
                        .lock()
                        .unwrap()
                        .as_deref()
                        .unwrap()
                        .contains("format changed")
                );
            } else {
                assert!(samples > 16000);
            }
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
}
