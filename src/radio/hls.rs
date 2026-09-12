//! A bounded, sequential HLS audio receiver (RFC 8216).
use super::{Input, MAX_PLAYLIST};
use crate::{Result, config::validate_url};
use cbc::cipher::{BlockDecryptMut, KeyIvInit, block_padding::Pkcs7};
use std::{
    collections::BTreeMap,
    io::{Cursor, Read},
    thread,
    time::{Duration, Instant},
};
use ureq::ResponseExt;
use url::Url;
mod ts;
const MAX_SEGMENT: usize = 16 * 1024 * 1024;
const MAX_MAP: usize = 1024 * 1024;

#[derive(Clone, Debug, PartialEq)]
struct Resource {
    url: Url,
    range: Option<(u64, u64)>,
}
#[derive(Clone, Debug)]
struct Key {
    url: Url,
    iv: Option<[u8; 16]>,
}
#[derive(Clone, Debug)]
struct Init {
    resource: Resource,
    key: Option<Key>,
}
#[derive(Clone, Debug)]
struct Segment {
    sequence: u64,
    resource: Resource,
    key: Option<Key>,
    init: Option<Init>,
    gap: bool,
}
#[derive(Debug)]
pub struct Media {
    segments: Vec<Segment>,
    target: Duration,
    ended: bool,
}
pub enum Playlist {
    Master(Vec<Url>),
    Media(Media),
}
fn attr(text: &str) -> Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    let mut rest = text.trim();
    while !rest.is_empty() {
        let (name, value) = rest.split_once('=').ok_or("invalid HLS attribute")?;
        let (value, tail) = if let Some(value) = value.strip_prefix('"') {
            let (value, tail) = value.split_once('"').ok_or("unterminated HLS attribute")?;
            (value, tail)
        } else {
            value
                .find(',')
                .map_or((value, ""), |index| value.split_at(index))
        };
        if out
            .insert(name.trim().into(), value.trim().into())
            .is_some()
        {
            return Err("duplicate HLS attribute".into());
        }
        rest = if tail.is_empty() {
            ""
        } else {
            tail.strip_prefix(',')
                .ok_or("invalid HLS attribute separator")?
                .trim()
        };
    }
    Ok(out)
}
fn uri(base: &Url, text: &str) -> Result<Url> {
    validate_url(base.join(text)?.as_str())
}
fn iv(text: &str) -> Result<[u8; 16]> {
    let hex = text
        .strip_prefix("0x")
        .or_else(|| text.strip_prefix("0X"))
        .ok_or("HLS IV must be hexadecimal")?;
    if hex.is_empty() || hex.len() > 32 {
        return Err("invalid HLS IV length".into());
    }
    let value = u128::from_str_radix(hex, 16)?;
    Ok(value.to_be_bytes())
}
fn range(text: &str, previous: Option<&Resource>, url: &Url) -> Result<(u64, u64)> {
    let (length, offset) = text
        .split_once('@')
        .map_or((text, None), |(n, o)| (n, Some(o)));
    let length: u64 = length.parse()?;
    if length == 0 || length > MAX_SEGMENT as u64 {
        return Err("invalid HLS byte range length".into());
    }
    let offset = match offset {
        Some(o) => o.parse()?,
        None => previous
            .filter(|r| r.url == *url)
            .and_then(|r| r.range)
            .and_then(|(o, n)| o.checked_add(n))
            .ok_or("implicit HLS byte range needs a preceding range on the same URI")?,
    };
    offset
        .checked_add(length)
        .ok_or("HLS byte range overflow")?;
    Ok((offset, length))
}
pub fn parse(text: &str, base: &Url) -> Result<Playlist> {
    if text.len() > MAX_PLAYLIST as usize
        || !text.trim_start_matches('\u{feff}').starts_with("#EXTM3U")
    {
        return Err("invalid HLS playlist".into());
    }
    let mut segments = Vec::new();
    let mut variants = Vec::new();
    let mut renditions: BTreeMap<String, Vec<(bool, Url)>> = BTreeMap::new();
    let (mut sequence, mut target, mut ended, mut duration) = (0u64, None, false, false);
    let (mut key, mut init, mut byterange, mut variant) = (None, None, None, None);
    let mut gap = false;
    for line in text.lines().map(str::trim).filter(|s| !s.is_empty()) {
        if let Some(v) = line.strip_prefix("#EXT-X-MEDIA:") {
            let a = attr(v)?;
            if a.get("TYPE").is_some_and(|v| v == "AUDIO")
                && let (Some(group), Some(path)) = (a.get("GROUP-ID"), a.get("URI"))
            {
                renditions.entry(group.clone()).or_default().push((
                    a.get("DEFAULT").is_some_and(|s| s == "YES"),
                    uri(base, path)?,
                ));
            }
        } else if let Some(v) = line.strip_prefix("#EXT-X-STREAM-INF:") {
            variant = Some(attr(v)?);
        } else if let Some(v) = line.strip_prefix("#EXT-X-TARGETDURATION:") {
            let n: u64 = v.parse()?;
            if !(1..=3600).contains(&n) {
                return Err("invalid HLS target duration".into());
            }
            target = Some(Duration::from_secs(n));
        } else if let Some(v) = line.strip_prefix("#EXT-X-MEDIA-SEQUENCE:") {
            if !segments.is_empty() {
                return Err("late HLS media sequence".into());
            }
            sequence = v.parse()?;
        } else if let Some(v) = line.strip_prefix("#EXTINF:") {
            let n: f64 = v.split(',').next().unwrap_or("").parse()?;
            if !n.is_finite() || n <= 0.0 || n > 3600.0 {
                return Err("invalid HLS segment duration".into());
            }
            duration = true;
        } else if let Some(v) = line.strip_prefix("#EXT-X-BYTERANGE:") {
            byterange = Some(v.to_string());
        } else if let Some(v) = line.strip_prefix("#EXT-X-KEY:") {
            let a = attr(v)?;
            key = match a.get("METHOD").map(String::as_str) {
                Some("NONE") => None,
                Some("AES-128") if a.get("KEYFORMAT").is_none_or(|f| f == "identity") => {
                    Some(Key {
                        url: uri(base, a.get("URI").ok_or("HLS key missing URI")?)?,
                        iv: a.get("IV").map(|s| iv(s)).transpose()?,
                    })
                }
                _ => {
                    return Err(
                        "unsupported HLS encryption (only identity AES-128 is supported)".into(),
                    );
                }
            };
        } else if let Some(v) = line.strip_prefix("#EXT-X-MAP:") {
            let a = attr(v)?;
            let url = uri(base, a.get("URI").ok_or("HLS map missing URI")?)?;
            if key.as_ref().is_some_and(|k| k.iv.is_none()) {
                return Err("encrypted HLS map requires explicit IV".into());
            }
            init = Some(Init {
                resource: Resource {
                    range: a
                        .get("BYTERANGE")
                        .map(|v| range(v, None, &url))
                        .transpose()?,
                    url,
                },
                key: key.clone(),
            });
        } else if line == "#EXT-X-ENDLIST" {
            ended = true;
        } else if line == "#EXT-X-GAP" {
            gap = true;
        } else if line.starts_with("#EXT-X-SKIP:")
            || line.starts_with("#EXT-X-PART:")
            || line == "#EXT-X-I-FRAMES-ONLY"
        {
            return Err("low-latency/delta or I-frame-only HLS is not supported".into());
        } else if !line.starts_with('#') {
            let url = uri(base, line)?;
            if let Some(a) = variant.take() {
                let bandwidth = a
                    .get("BANDWIDTH")
                    .ok_or("HLS variant missing bandwidth")?
                    .parse::<u64>()?;
                variants.push((
                    a.contains_key("RESOLUTION"),
                    bandwidth,
                    a.get("AUDIO").cloned(),
                    url,
                ));
            } else {
                if !duration {
                    return Err("HLS segment missing EXTINF".into());
                }
                let previous = segments.last().map(|s: &Segment| &s.resource);
                let resource = Resource {
                    range: byterange
                        .take()
                        .map(|r| range(&r, previous, &url))
                        .transpose()?,
                    url,
                };
                segments.push(Segment {
                    sequence,
                    resource,
                    key: key.clone(),
                    init: init.clone(),
                    gap,
                });
                sequence = sequence.checked_add(1).ok_or("HLS sequence overflow")?;
                duration = false;
                gap = false;
            }
        }
    }
    if duration || variant.is_some() || byterange.is_some() {
        return Err("unfinished HLS segment/variant".into());
    }
    if !variants.is_empty() {
        if !segments.is_empty() {
            return Err("mixed HLS master and media playlist".into());
        }
        variants.sort_by_key(|v| (v.0, v.1));
        let mut urls = Vec::new();
        for (_, _, group, url) in variants {
            if let Some(group) = group
                && let Some(audio) = renditions.get_mut(&group)
            {
                audio.sort_by_key(|(default, _)| !default);
                urls.extend(audio.iter().map(|(_, url)| url.clone()));
            }
            urls.push(url);
        }
        urls.dedup();
        Ok(Playlist::Master(urls))
    } else {
        Ok(Playlist::Media(Media {
            segments,
            target: target.ok_or("HLS media playlist missing target duration")?,
            ended,
        }))
    }
}

fn download(agent: &ureq::Agent, resource: &Resource, max: usize) -> Result<Vec<u8>> {
    let mut request = agent
        .get(resource.url.as_str())
        .header("Accept-Encoding", "identity");
    if let Some((offset, length)) = resource.range {
        if length > max as u64 {
            return Err("HLS resource exceeds size limit".into());
        }
        request = request.header("Range", format!("bytes={offset}-{}", offset + length - 1));
    }
    let response = request
        .config()
        .timeout_recv_body(Some(Duration::from_secs(30)))
        .build()
        .call()?;
    if let Some((offset, length)) = resource.range {
        let expected = format!("bytes {offset}-{}/", offset + length - 1);
        if response.status().as_u16() != 206
            || !response
                .headers()
                .get("content-range")
                .and_then(|h| h.to_str().ok())
                .is_some_and(|v| v.starts_with(&expected))
        {
            return Err("server did not honor HLS byte range".into());
        }
    }
    let mut bytes = Vec::new();
    response
        .into_body()
        .into_reader()
        .take(max as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > max {
        return Err("HLS resource exceeds size limit".into());
    }
    if resource.range.is_some_and(|(_, n)| n != bytes.len() as u64) {
        return Err("truncated HLS byte range".into());
    }
    Ok(bytes)
}
fn decrypt(
    agent: &ureq::Agent,
    mut data: Vec<u8>,
    key: &Option<Key>,
    sequence: u64,
) -> Result<Vec<u8>> {
    if let Some(key) = key {
        let bytes = download(
            agent,
            &Resource {
                url: key.url.clone(),
                range: None,
            },
            16,
        )?;
        let bytes: [u8; 16] = bytes
            .try_into()
            .map_err(|_| "HLS AES key must be 16 bytes")?;
        let iv = key.iv.unwrap_or_else(|| (sequence as u128).to_be_bytes());
        let plaintext = cbc::Decryptor::<aes::Aes128>::new(&bytes.into(), &iv.into())
            .decrypt_padded_mut::<Pkcs7>(&mut data)
            .map_err(|_| "invalid AES-128 HLS segment padding")?;
        let len = plaintext.len();
        data.truncate(len);
    }
    Ok(data)
}
pub struct Session {
    url: Url,
    media: Media,
    next: u64,
    last_progress: Instant,
    last_reload: Instant,
}
impl Session {
    pub fn checkpoint(&self) -> (Url, u64) {
        (self.url.clone(), self.next)
    }
    pub fn resume(&mut self, checkpoint: &(Url, u64)) {
        if self.url == checkpoint.0
            && self
                .media
                .segments
                .last()
                .is_none_or(|s| checkpoint.1 <= s.sequence + 1)
        {
            self.next = checkpoint.1;
        }
    }
    pub fn new(url: Url, media: Media) -> Self {
        let start = if media.ended {
            0
        } else {
            media.segments.len().saturating_sub(3)
        };
        let next = media.segments.get(start).map_or(0, |s| s.sequence);
        Self {
            url,
            media,
            next,
            last_progress: Instant::now(),
            last_reload: Instant::now(),
        }
    }
    pub fn next(
        &mut self,
        agent: &ureq::Agent,
        active: &dyn Fn() -> bool,
    ) -> Result<Option<Input>> {
        loop {
            if !active() {
                return Ok(None);
            }
            if let Some(segment) = self
                .media
                .segments
                .iter()
                .find(|s| s.sequence >= self.next)
                .cloned()
            {
                if segment.gap {
                    self.next = segment.sequence + 1;
                    continue;
                }
                let bytes = download(agent, &segment.resource, MAX_SEGMENT)?;
                let mut bytes = decrypt(agent, bytes, &segment.key, segment.sequence)?;
                if let Some(init) = &segment.init {
                    let map = download(agent, &init.resource, MAX_MAP)?;
                    let mut map = decrypt(agent, map, &init.key, segment.sequence)?;
                    map.extend(bytes);
                    bytes = map;
                } else if bytes.first() == Some(&0x47) {
                    bytes = ts::extract(&bytes)?;
                }
                self.next = segment.sequence + 1;
                self.last_progress = Instant::now();
                return Ok(Some(Input {
                    byte_len: Some(bytes.len() as u64),
                    data: Box::new(Cursor::new(bytes)),
                    mime: String::new(),
                    interval: None,
                    finite: true,
                }));
            }
            if self.media.ended {
                return Ok(None);
            }
            if self.last_progress.elapsed()
                > self
                    .media
                    .target
                    .saturating_mul(3)
                    .max(Duration::from_secs(30))
            {
                return Err("HLS playlist stopped advancing".into());
            }
            let wait = self.media.target / 2;
            while self.last_reload.elapsed() < wait {
                if !active() {
                    return Ok(None);
                }
                thread::sleep(Duration::from_millis(50));
            }
            let response = agent
                .get(self.url.as_str())
                .config()
                .timeout_recv_body(Some(Duration::from_secs(30)))
                .build()
                .call()?;
            let base = validate_url(&response.get_uri().to_string())?;
            let text = super::read_playlist(response.into_body().into_reader(), active)?;
            let parsed = parse(&text, &base)?;
            let Playlist::Media(media) = parsed else {
                return Err("HLS media playlist changed into a master".into());
            };
            self.url = base;
            self.media = media;
            self.last_reload = Instant::now();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn base() -> Url {
        Url::parse("https://example.org/radio/live.m3u8").unwrap()
    }
    #[test]
    fn parses_ranges_keys_maps_and_quoted_attributes() {
        let a = attr("CODECS=\"avc1,mp4a\",BANDWIDTH=42").unwrap();
        assert_eq!(a["CODECS"], "avc1,mp4a");
        let text = "#EXTM3U\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:7\n#EXT-X-KEY:METHOD=AES-128,URI=\"key\",IV=0x02\n#EXT-X-MAP:URI=\"init\",BYTERANGE=\"100@0\"\n#EXT-X-BYTERANGE:32@0\n#EXTINF:1,\nall\n#EXT-X-KEY:METHOD=NONE\n#EXT-X-BYTERANGE:32\n#EXTINF:1,\nall\n#EXT-X-ENDLIST";
        let Playlist::Media(media) = parse(text, &base()).unwrap() else {
            panic!()
        };
        assert_eq!(media.segments[0].sequence, 7);
        assert_eq!(media.segments[1].resource.range, Some((32, 32)));
        assert!(media.segments[1].key.is_none());
        assert_eq!(
            media.segments[1]
                .init
                .as_ref()
                .unwrap()
                .key
                .as_ref()
                .unwrap()
                .iv
                .unwrap()[15],
            2
        );
    }
    #[test]
    fn rejects_unsupported_or_malformed_hls() {
        for extra in [
            "#EXT-X-KEY:METHOD=SAMPLE-AES,URI=\"key\"",
            "#EXT-X-PART:DURATION=1,URI=\"part\"",
            "#EXT-X-KEY:METHOD=AES-128,URI=\"key\"\n#EXT-X-MAP:URI=\"init\"",
            "#EXT-X-BYTERANGE:16",
            "#EXT-X-BYTERANGE:16@18446744073709551615",
        ] {
            assert!(
                parse(
                    &format!("#EXTM3U\n#EXT-X-TARGETDURATION:1\n{extra}\n#EXTINF:1,\nsegment"),
                    &base()
                )
                .is_err(),
                "{extra}"
            );
        }
        assert!(attr("URI=\"unfinished").is_err());
        assert!(attr("URI=a,URI=b").is_err());
        assert!(iv("0xnothex").is_err());
        assert!(parse("#EXTM3U\n#EXT-X-TARGETDURATION:0", &base()).is_err());
    }
    #[test]
    fn idle_live_wait_is_cancellable() {
        let Playlist::Media(media) = parse("#EXTM3U\n#EXT-X-TARGETDURATION:10", &base()).unwrap()
        else {
            panic!()
        };
        let mut session = Session::new(base(), media);
        let start = Instant::now();
        let active = || start.elapsed() < Duration::from_millis(100);
        assert!(
            session
                .next(&super::super::agent(Duration::from_secs(1)), &active)
                .unwrap()
                .is_none()
        );
        assert!(start.elapsed() < Duration::from_secs(1));
    }
    #[test]
    fn rejects_bad_transport_stream_framing() {
        assert!(ts::extract(&[]).is_err());
        assert!(ts::extract(&[0x47; 187]).is_err());
        assert!(ts::extract(&[0; 188]).is_err());
    }
}
