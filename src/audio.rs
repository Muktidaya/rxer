//! HTTP and decoding run off the audio callback. A bounded PCM queue limits memory.
use crate::Result;
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
    channels: ChannelCount,
    rate: SampleRate,
    pub failure: Failure,
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
        match self.rx.try_recv() {
            Ok(chunk) => {
                self.pending = chunk.into_iter();
                self.pending.next()
            }
            // Keep the device callback nonblocking during a network stall.
            Err(TryRecvError::Empty) => {
                self.silence_left = self.channels.get() as usize - 1;
                Some(0.0)
            }
            Err(TryRecvError::Disconnected) => None,
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

pub fn connect(url: &str, stopped: &Arc<AtomicBool>) -> Result<Option<Audio>> {
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let (tx, rx) = mpsc::sync_channel(8);
    let failure: Failure = Arc::new(Mutex::new(None));
    let worker_failure = failure.clone();
    let cancelled = stopped.clone();
    let url = url.to_string();
    thread::spawn(move || {
        let run = || -> Result<()> {
            let agent: ureq::Agent = ureq::Agent::config_builder()
                .timeout_connect(Some(Duration::from_secs(10)))
                .timeout_recv_response(Some(Duration::from_secs(15)))
                .build()
                .into();
            let response = agent.get(&url).header("Icy-MetaData", "0").call()?;
            if response.headers().get("icy-metaint").is_some() {
                return Err(
                    "server sends ICY metadata despite opt-out; use a direct audio stream".into(),
                );
            }
            let mime = response
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_string();
            let reader = StreamReader {
                inner: response.into_body().into_reader(),
                failure: worker_failure.clone(),
            };
            let mut decoder = Decoder::builder()
                .with_data(reader)
                .with_seekable(false)
                .with_mime_type(&mime)
                .build()?;
            let channels = decoder.channels();
            let rate = decoder.sample_rate();
            ready_tx.send((channels, rate))?;
            loop {
                if cancelled.load(Ordering::Relaxed) {
                    return Ok(());
                }
                let chunk: Vec<_> = decoder
                    .by_ref()
                    .take(2048 * channels.get() as usize)
                    .collect();
                if chunk.is_empty() {
                    return Ok(());
                }
                if decoder.channels() != channels || decoder.sample_rate() != rate {
                    return Err("stream audio format changed; restart rxer".into());
                }
                if tx.send(chunk).is_err() {
                    return Ok(());
                }
            }
        };
        if let Err(error) = run()
            && let Ok(mut failure) = worker_failure.lock()
        {
            *failure = Some(error.to_string());
        }
    });
    loop {
        if stopped.load(Ordering::Relaxed) {
            return Ok(None);
        }
        match ready_rx.recv_timeout(Duration::from_millis(100)) {
            Ok((channels, rate)) => {
                return Ok(Some(Audio {
                    rx,
                    pending: Vec::new().into_iter(),
                    silence_left: 0,
                    channels,
                    rate,
                    failure,
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
            channels: ChannelCount::new(2).unwrap(),
            rate: SampleRate::new(8000).unwrap(),
            failure: Arc::new(Mutex::new(None)),
        };
        assert_eq!(audio.next(), Some(1.0));
        assert_eq!(audio.next(), Some(2.0));
        assert_eq!(audio.next(), Some(0.0));
        tx.send(vec![3.0, 4.0]).unwrap();
        // A refill between left and right must not shift channel assignment.
        assert_eq!(audio.next(), Some(0.0));
        drop(tx);
        assert_eq!(audio.collect::<Vec<_>>(), vec![3.0, 4.0]);
    }
}
