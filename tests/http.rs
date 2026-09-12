use std::{
    io::{Read, Write},
    net::TcpListener,
    process::Command,
    thread,
};

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
    let out = Command::new(env!("CARGO_BIN_EXE_rxer"))
        .args(["--check", &url])
        .output()
        .unwrap();
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
        let out = Command::new(env!("CARGO_BIN_EXE_rxer"))
            .args(["--check", &url])
            .output()
            .unwrap();
        assert!(!out.status.success());
        assert!(!out.stderr.is_empty());
    }
}
