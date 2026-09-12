use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

pub struct Response {
    pub status: &'static str,
    pub headers: String,
    pub body: Vec<u8>,
    pub fragment: bool,
}
impl Response {
    pub fn new(body: impl Into<Vec<u8>>, mime: &str) -> Self {
        Self {
            status: "HTTP/1.1 200 OK",
            headers: format!("Content-Type: {mime}\r\n"),
            body: body.into(),
            fragment: false,
        }
    }
}
pub struct Server {
    pub url: String,
    pub requests: Arc<Mutex<Vec<String>>>,
    stopped: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Server {
    pub fn new(handler: impl Fn(&str, &str, usize) -> Response + Send + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stopped = Arc::new(AtomicBool::new(false));
        let (log, stop) = (requests.clone(), stopped.clone());
        let worker = thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                let mut stream = match listener.accept() {
                    Ok((s, _)) => s,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(e) => panic!("{e}"),
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut bytes = Vec::new();
                while !bytes.ends_with(b"\r\n\r\n") && bytes.len() < 16384 {
                    let mut byte = [0];
                    if stream.read_exact(&mut byte).is_err() {
                        break;
                    }
                    bytes.push(byte[0]);
                }
                let request = String::from_utf8_lossy(&bytes);
                let path = request.split_whitespace().nth(1).unwrap_or("");
                let count = {
                    let mut log = log.lock().unwrap();
                    log.push(path.to_string());
                    log.iter().filter(|p| *p == path).count()
                };
                let response = handler(path, &request, count);
                let header = format!(
                    "{}\r\n{}Content-Length: {}\r\nConnection: close\r\n\r\n",
                    response.status,
                    response.headers,
                    response.body.len()
                );
                if response.fragment {
                    for byte in header.bytes() {
                        if stream.write_all(&[byte]).is_err() {
                            break;
                        }
                        thread::sleep(Duration::from_millis(1));
                    }
                } else {
                    let _ = stream.write_all(header.as_bytes());
                }
                let _ = stream.write_all(&response.body);
            }
        });
        Self {
            url,
            requests,
            stopped,
            worker: Some(worker),
        }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Relaxed);
        let result = self.worker.take().unwrap().join();
        if !thread::panicking() {
            result.unwrap();
        }
    }
}
