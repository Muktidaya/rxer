//! Preserve ureq's TLS/proxy connector while adapting legacy ICY status lines.
use std::time::Duration;
use ureq::unversioned::{
    resolver::DefaultResolver,
    transport::{
        Buffers, ConnectionDetails, Connector, DefaultConnector, LazyBuffers, NextTimeout,
        Transport,
    },
};

#[derive(Debug)]
struct RadioConnector(Duration);
#[derive(Debug)]
struct RadioTransport<T> {
    inner: T,
    idle: Duration,
    buffers: LazyBuffers,
    prefix: Vec<u8>,
    started: bool,
}
impl<T: Transport> Connector<T> for RadioConnector {
    type Out = RadioTransport<T>;
    fn connect(
        &self,
        _: &ConnectionDetails,
        transport: Option<T>,
    ) -> Result<Option<Self::Out>, ureq::Error> {
        Ok(transport.map(|inner| RadioTransport {
            inner,
            idle: self.0,
            buffers: LazyBuffers::new(32 * 1024, 16 * 1024),
            prefix: Vec::with_capacity(9),
            started: false,
        }))
    }
}
impl<T: Transport> Transport for RadioTransport<T> {
    fn buffers(&mut self) -> &mut dyn Buffers {
        &mut self.buffers
    }
    fn transmit_output(&mut self, amount: usize, timeout: NextTimeout) -> Result<(), ureq::Error> {
        self.inner.buffers().output()[..amount].copy_from_slice(&self.buffers.output()[..amount]);
        self.inner.transmit_output(amount, timeout)
    }
    fn await_input(&mut self, mut timeout: NextTimeout) -> Result<bool, ureq::Error> {
        if timeout.after > self.idle.into() {
            timeout.after = self.idle.into();
            timeout.reason = ureq::Timeout::RecvBody;
        }
        loop {
            if !self.inner.maybe_await_input(timeout)? {
                return Ok(false);
            }
            let input = self.inner.buffers().input();
            if !self.started {
                let n = (4 - self.prefix.len()).min(input.len());
                self.prefix.extend_from_slice(&input[..n]);
                self.inner.buffers().input_consume(n);
                if self.prefix.len() < 4 {
                    continue;
                }
                let prefix = if self.prefix == b"ICY " {
                    b"HTTP/1.0 ".as_slice()
                } else {
                    &self.prefix
                };
                self.buffers.input_append_buf()[..prefix.len()].copy_from_slice(prefix);
                self.buffers.input_appended(prefix.len());
                self.started = true;
                return Ok(true);
            }
            let n = input.len().min(self.buffers.input_append_buf().len());
            self.buffers.input_append_buf()[..n].copy_from_slice(&input[..n]);
            self.buffers.input_appended(n);
            self.inner.buffers().input_consume(n);
            return Ok(true);
        }
    }
    fn is_open(&mut self) -> bool {
        self.inner.is_open()
    }
    fn is_tls(&self) -> bool {
        self.inner.is_tls()
    }
}
pub fn agent(idle: Duration) -> ureq::Agent {
    let config = ureq::Agent::config_builder()
        .timeout_resolve(Some(Duration::from_secs(10)))
        .timeout_connect(Some(Duration::from_secs(10)))
        .timeout_send_request(Some(Duration::from_secs(10)))
        .timeout_recv_response(Some(Duration::from_secs(15)))
        .max_redirects(5)
        .input_buffer_size(16 * 1024)
        .output_buffer_size(16 * 1024)
        // One response per transport makes prefix handling independent of pooling.
        .max_idle_connections(0)
        .build();
    ureq::Agent::with_parts(
        config,
        DefaultConnector::default().chain(RadioConnector(idle)),
        DefaultResolver::default(),
    )
}
