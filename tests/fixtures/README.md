# Synthetic radio fixtures

These files contain a generated 440 Hz sine wave, not recorded or third-party
programming. They are project test data under the repository license. The tests
read these checked-in files and do not require FFmpeg or network access beyond
their local fixture servers.

Generated with FFmpeg 9.0.1 using these commands in an empty directory:

```sh
ffmpeg -f lavfi -i sine=frequency=440:sample_rate=16000 -t 2.1 -c:a aac -b:a 24k -f adts tone.aac
ffmpeg -f lavfi -i sine=frequency=440:sample_rate=16000 -t 2.1 -c:a aac -b:a 24k -f mpegts tone.ts
ffmpeg -f lavfi -i sine=frequency=440:sample_rate=16000 -t 2.1 -c:a libmp3lame -b:a 24k tone.mp3
ffmpeg -f lavfi -i sine=frequency=440:sample_rate=16000 -t 2.1 -c:a aac -b:a 24k -f hls -hls_time 1 -hls_playlist_type vod -hls_segment_type fmp4 -hls_segment_filename part%02d.m4s media.m3u8
```

The last command also writes `init.mp4`. Encoded bytes may change with FFmpeg
versions; tests assert decoding behavior and timing bounds, not encoder byte
identity. AES test ciphertext is derived from `tone.aac` with synthetic test keys
at test time.
