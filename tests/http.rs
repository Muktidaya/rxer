use std::{
    io::{Read, Write},
    net::TcpListener,
    process::Command,
    thread,
};

fn rxer() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_rxer"));
    command
        .arg("--config")
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/defaults.toml"));
    command
}

fn wav() -> Vec<u8> {
    let data_len = 16000u32 * 2;
    let mut b = Vec::new();
    b.extend(b"RIFF");
    b.extend((36 + data_len).to_le_bytes());
    b.extend(b"WAVEfmt ");
    b.extend(16u32.to_le_bytes());
    b.extend(1u16.to_le_bytes());
    b.extend(1u16.to_le_bytes());
    b.extend(8000u32.to_le_bytes());
    b.extend(16000u32.to_le_bytes());
    b.extend(2u16.to_le_bytes());
    b.extend(16u16.to_le_bytes());
    b.extend(b"data");
    b.extend(data_len.to_le_bytes());
    for i in 0..16000 {
        b.extend(((i % 80) as i16 * 100 - 4000).to_le_bytes());
    }
    b
}

fn serve(status: &str, content_type: &str, body: Vec<u8>, redirect: bool) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let header = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    thread::spawn(move || {
        for n in 0..if redirect { 2 } else { 1 } {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 4096];
            let _ = stream.read(&mut request);
            if redirect && n == 0 {
                write!(stream, "HTTP/1.1 302 Found\r\nLocation: http://{address}/audio\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
            } else {
                stream.write_all(header.as_bytes()).unwrap();
                let _ = stream.write_all(&body);
            }
        }
    });
    format!("http://{address}/stream")
}

#[test]
fn decodes_http_audio_after_redirect_without_device() {
    let url = serve("200 OK", "audio/wav", wav(), true);
    let out = rxer().args(["--check", &url]).output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("8000 Hz, 1 channel(s)"));
}

#[test]
fn rejects_http_errors_and_non_audio() {
    for (status, body) in [
        ("404 Not Found", b"missing".to_vec()),
        ("200 OK", b"<html>not audio</html>".to_vec()),
    ] {
        let url = serve(status, "text/html", body, false);
        let out = rxer().args(["--check", &url]).output().unwrap();
        assert!(!out.status.success());
        assert!(!out.stderr.is_empty());
    }
}

#[test]
fn follows_nested_pls_m3u_and_strips_icy_before_decoding() {
    let audio = wav();
    let mut icy = Vec::new();
    for chunk in audio.chunks(1024) {
        icy.extend(chunk);
        if chunk.len() == 1024 {
            let mut metadata = b"StreamTitle='test';StreamUrl='https://example.org';".to_vec();
            metadata.resize(64, 0);
            icy.push(4);
            icy.extend(metadata);
        }
    }
    let stream = serve("200 OK", "audio/wav\r\nicy-metaint: 1024", icy, false);
    let m3u = serve(
        "200 OK",
        "audio/x-mpegurl",
        format!("#EXTM3U\n{stream}\n").into_bytes(),
        false,
    );
    let pls = serve(
        "200 OK",
        "audio/x-scpls",
        format!("[playlist]\nFile1={m3u}\nNumberOfEntries=1\n").into_bytes(),
        false,
    );
    let out = rxer().args(["--check", &pls]).output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
#[test]
fn rejects_hls_oversized_empty_playlists_and_invalid_icy() {
    for (mime, body) in [
        (
            "audio/x-mpegurl",
            b"#EXTM3U\n#EXT-X-TARGETDURATION:10\nsegment.aac".to_vec(),
        ),
        ("audio/x-scpls", vec![b'x'; 65537]),
        ("audio/x-mpegurl", b"#EXTM3U\n".to_vec()),
        ("audio/wav\r\nicy-metaint: 0", wav()),
        ("audio/wav\r\nicy-metaint: invalid", wav()),
    ] {
        let url = serve("200 OK", mime, body, false);
        let out = rxer().args(["--check", &url]).output().unwrap();
        assert!(!out.status.success());
    }
}

#[test]
fn resolves_relative_playlist_entry_against_redirect_destination() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        for path in ["/start", "/nested/list.m3u", "/nested/audio"] {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(3)))
                .unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                let mut b = [0];
                stream.read_exact(&mut b).unwrap();
                request.push(b[0]);
            }
            let request = String::from_utf8(request).unwrap();
            assert!(request.starts_with(&format!("GET {path} ")), "{request}");
            assert!(request.to_ascii_lowercase().contains("icy-metadata: 1"));
            if path == "/start" {
                write!(stream, "HTTP/1.1 302 Found\r\nLocation: http://{address}/nested/list.m3u\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
            } else {
                let body = if path.ends_with(".m3u") {
                    b"#EXTM3U\naudio\n".to_vec()
                } else {
                    wav()
                };
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).unwrap();
                let _ = stream.write_all(&body);
            }
        }
    });
    let out = rxer()
        .args(["--check", &format!("http://{address}/start")])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    server.join().unwrap();
}
#[test]
fn rejects_playlist_cycles() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let _ = stream.read(&mut [0; 4096]);
        let body = format!("http://{address}/list.m3u");
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
    });
    let out = rxer()
        .args(["--check", &format!("http://{address}/list.m3u")])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("cycle"));
    server.join().unwrap();
}
