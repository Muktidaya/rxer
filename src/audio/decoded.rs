//! Give the resampler accurate, nonempty decoder span boundaries.
use rodio::{ChannelCount, Decoder, SampleRate, Source};
use std::{
    io::{Read, Seek},
    time::Duration,
};

pub struct PrimedDecoder<R: Read + Seek> {
    inner: Decoder<R>,
    next: Option<f32>,
    remaining: usize,
    channels: ChannelCount,
    rate: SampleRate,
}
impl<R: Read + Seek> PrimedDecoder<R> {
    pub fn new(mut inner: Decoder<R>) -> Self {
        // MP3 encoder-delay frames can decode to an empty initial buffer.
        // Decoder::next skips them; UniformSourceIterator cannot advance a zero span.
        let next = inner.next();
        let channels = inner.channels();
        let rate = inner.sample_rate();
        // rodio's Symphonia decoder reports the full current packet buffer length,
        // not the remaining length. The first sample is held here, still unconsumed.
        let remaining = if next.is_some() {
            inner.current_span_len().unwrap_or(1).max(1)
        } else {
            0
        };
        Self {
            inner,
            next,
            remaining,
            channels,
            rate,
        }
    }
}
impl<R: Read + Seek> Iterator for PrimedDecoder<R> {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        let sample = self.next.take()?;
        self.remaining -= 1;
        self.next = self.inner.next();
        if self.next.is_none() {
            self.remaining = 0;
        } else if self.remaining == 0 {
            self.remaining = self.inner.current_span_len().unwrap_or(1).max(1);
            self.channels = self.inner.channels();
            self.rate = self.inner.sample_rate();
        }
        Some(sample)
    }
}
impl<R: Read + Seek> Source for PrimedDecoder<R> {
    fn current_span_len(&self) -> Option<usize> {
        Some(self.remaining)
    }
    fn channels(&self) -> ChannelCount {
        self.channels
    }
    fn sample_rate(&self) -> SampleRate {
        self.rate
    }
    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }
}
