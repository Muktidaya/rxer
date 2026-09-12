mod support;
use std::process::{Command, Output};
use support::{Response, Server};
const AAC: &[u8] = include_bytes!("fixtures/tone.aac");
const MP3: &[u8] = include_bytes!("fixtures/tone.mp3");
const TS: &[u8] = include_bytes!("fixtures/tone.ts");
const INIT: &[u8] = include_bytes!("fixtures/init.mp4");
const PART: &[u8] = include_bytes!("fixtures/part00.m4s");
fn check(server: &Server, path: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rxer"))
        .args([
            "--config",
            concat!(env!("CARGO_MANIFEST_DIR"), "/assets/defaults.toml"),
            "--check",
            &format!("{}{path}", server.url),
        ])
        .output()
        .unwrap()
}
fn success(output: Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("16000 Hz"));
}
fn media(path: &str, extra: &str) -> String {
    format!("#EXTM3U\n#EXT-X-TARGETDURATION:3\n{extra}\n#EXTINF:2.1,\n{path}\n#EXT-X-ENDLIST\n")
}
#[test]
fn plays_packed_aac_mp3_transport_stream_and_fragmented_mp4_hls() {
    for (name, bytes) in [("aac", AAC), ("mp3", MP3), ("ts", TS), ("m4s", PART)] {
        let server = Server::new(move |path, _, _| match path {
            "/radio.m3u8" => Response::new(
                media(
                    &format!("/audio.{name}"),
                    if name == "m4s" {
                        "#EXT-X-MAP:URI=\"/init.mp4\""
                    } else {
                        ""
                    },
                ),
                "application/vnd.apple.mpegurl",
            ),
            "/init.mp4" => Response::new(INIT, "video/mp4"),
            _ => Response::new(bytes, "application/octet-stream"),
        });
        let out = check(&server, "/radio.m3u8");
        success(out);
        assert!(
            server
                .requests
                .lock()
                .unwrap()
                .contains(&format!("/audio.{name}"))
        );
    }
}
#[test]
fn follows_audio_rendition_and_fails_over_http_and_decoder_errors() {
    let server = Server::new(|path, _, _| match path {
        "/list.pls" => Response::new(
            "[playlist]\nFile1=/missing\nFile2=/junk\nFile3=/master.m3u8",
            "audio/x-scpls",
        ),
        "/missing" => {
            let mut r = Response::new("missing", "text/plain");
            r.status = "HTTP/1.1 404 Not Found";
            r
        }
        "/junk" => Response::new("not audio", "audio/aac"),
        "/master.m3u8" => Response::new(
            "#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"radio\",NAME=\"a,b\",DEFAULT=YES,URI=\"audio.m3u8\"\n#EXT-X-STREAM-INF:BANDWIDTH=100000,CODECS=\"avc1.42e01e,mp4a.40.2\",AUDIO=\"radio\"\nvideo.m3u8",
            "application/vnd.apple.mpegurl",
        ),
        "/audio.m3u8" => Response::new(media("tone.aac", ""), "application/vnd.apple.mpegurl"),
        "/tone.aac" => Response::new(AAC, "audio/aac"),
        _ => panic!("unexpected request {path}"),
    });
    success(check(&server, "/list.pls"));
    assert_eq!(
        *server.requests.lock().unwrap(),
        [
            "/list.pls",
            "/missing",
            "/junk",
            "/master.m3u8",
            "/audio.m3u8",
            "/tone.aac"
        ]
    );
}
#[test]
fn plays_fragmented_legacy_icy_status_line() {
    let server = Server::new(|_, _, _| {
        let mut response = Response::new(AAC, "audio/aac");
        response.status = "ICY 200 OK";
        response.fragment = true;
        response
    });
    success(check(&server, "/legacy"));
}
#[test]
fn decrypts_aes128_with_sequence_iv() {
    use cbc::cipher::{BlockEncryptMut, KeyIvInit, block_padding::Pkcs7};
    let key = [7u8; 16];
    let iv = 7u128.to_be_bytes();
    let mut data = AAC.to_vec();
    data.resize(data.len() + 16, 0);
    let data = cbc::Encryptor::<aes::Aes128>::new(&key.into(), &iv.into())
        .encrypt_padded_mut::<Pkcs7>(&mut data, AAC.len())
        .unwrap()
        .to_vec();
    let server = Server::new(move |path, _, _| match path {
        "/radio.m3u8" => Response::new(
            media(
                "encrypted",
                "#EXT-X-MEDIA-SEQUENCE:7\n#EXT-X-KEY:METHOD=AES-128,URI=\"key\"",
            ),
            "application/vnd.apple.mpegurl",
        ),
        "/key" => Response::new(key.to_vec(), "application/octet-stream"),
        "/encrypted" => Response::new(data.clone(), "application/octet-stream"),
        _ => panic!("unexpected request"),
    });
    success(check(&server, "/radio.m3u8"));
}
#[test]
fn requests_and_verifies_byte_ranges() {
    for honor in [true, false] {
        let server = Server::new(move |path, request, _| {
            if path == "/radio.m3u8" {
                return Response::new(
                    media("audio", &format!("#EXT-X-BYTERANGE:{}@8", AAC.len())),
                    "application/vnd.apple.mpegurl",
                );
            }
            assert!(
                request
                    .to_ascii_lowercase()
                    .contains(&format!("range: bytes=8-{}", AAC.len() + 7))
            );
            let mut response = Response::new(AAC, "audio/aac");
            if honor {
                response.status = "HTTP/1.1 206 Partial Content";
                response.headers.push_str(&format!(
                    "Content-Range: bytes 8-{}/{}\r\n",
                    AAC.len() + 7,
                    AAC.len() + 16
                ));
            }
            response
        });
        let out = check(&server, "/radio.m3u8");
        if honor {
            success(out);
        } else {
            assert!(!out.status.success());
            assert!(String::from_utf8_lossy(&out.stderr).contains("byte range"));
        }
    }
}

#[test]
fn exhausted_nested_branch_does_not_hide_a_working_alternative() {
    let server = Server::new(|path, _, _| {
        if path == "/root.m3u" {
            Response::new("/depth0.m3u\n/good.aac", "audio/x-mpegurl")
        } else if path == "/good.aac" {
            Response::new(AAC, "audio/aac")
        } else {
            let n = path
                .trim_start_matches("/depth")
                .trim_end_matches(".m3u")
                .parse::<usize>()
                .unwrap();
            Response::new(format!("/depth{}.m3u", n + 1), "audio/x-mpegurl")
        }
    });
    success(check(&server, "/root.m3u"));
    assert!(server.requests.lock().unwrap().len() <= 7);
}
