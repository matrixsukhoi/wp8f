use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream};
use std::sync::atomic::AtomicU64;
use std::time::Duration;
use thiserror::Error;

const DEFAULT_HOST: Ipv4Addr = Ipv4Addr::new(127, 0, 0, 1);
const DEFAULT_PORTS: &[u16] = &[8111, 9222];
const DEFAULT_TIMEOUT_MS: u64 = 100;

const ENDPOINT_STATE: &str = "/state";
const ENDPOINT_INDICATORS: &str = "/indicators";
const ENDPOINT_MAP_OBJ: &str = "/map_obj.json";
const ENDPOINT_MAP_INFO: &str = "/map_info.json";
const ENDPOINT_MAP_IMG: &str = "/map.img";

// 热路径告警限流：每个站点一个时间戳（见 `crate::warn_throttled`）。
static WARN_PARSE_STATE: AtomicU64 = AtomicU64::new(0);
static WARN_PARSE_INDIC: AtomicU64 = AtomicU64::new(0);
static WARN_PARSE_MAPOBJ: AtomicU64 = AtomicU64::new(0);
static WARN_READ_STATE: AtomicU64 = AtomicU64::new(0);
static WARN_READ_INDIC: AtomicU64 = AtomicU64::new(0);
static WARN_READ_MAPOBJ: AtomicU64 = AtomicU64::new(0);

#[derive(Error, Debug)]
pub enum ChannelError {
    #[error("Connection failed: {0}")]
    ConnectionFailed(String),
    #[error("Connection timeout")]
    Timeout,
    #[error("Invalid HTTP response")]
    InvalidResponse,
    #[error("Read error: {0}")]
    ReadError(String),
    #[error("Write error: {0}")]
    WriteError(String),
}

#[derive(Clone)]
pub struct ChannelConfig {
    pub host: Ipv4Addr,
    pub ports: Vec<u16>,
    pub timeout_ms: u64,
}

impl Default for ChannelConfig {
    fn default() -> Self {
        Self {
            host: DEFAULT_HOST,
            ports: DEFAULT_PORTS.to_vec(),
            timeout_ms: DEFAULT_TIMEOUT_MS,
        }
    }
}

impl ChannelConfig {
    pub fn new(host: Ipv4Addr, ports: Vec<u16>) -> Self {
        Self {
            host,
            ports: if ports.is_empty() { DEFAULT_PORTS.to_vec() } else { ports },
            timeout_ms: DEFAULT_TIMEOUT_MS,
        }
    }
}

pub struct Channel {
    config: ChannelConfig,
    current_port_index: usize,
    connected: bool,
}

impl Channel {
    pub fn new(config: ChannelConfig) -> Self {
        Self {
            config: config.clone(),
            current_port_index: 0,
            connected: false,
        }
    }


    pub fn with_ip_and_ports(host: Ipv4Addr, ports: Vec<u16>) -> Self {
        Self::new(ChannelConfig::new(host, ports))
    }

    fn current_port(&self) -> u16 {
        self.config.ports[self.current_port_index % self.config.ports.len()]
    }

    fn connect(&mut self) -> Result<TcpStream, ChannelError> {
        let addr = SocketAddr::new(IpAddr::V4(self.config.host), self.current_port());
        let stream =
            TcpStream::connect_timeout(&addr, Duration::from_millis(self.config.timeout_ms))
                .map_err(|e| ChannelError::ConnectionFailed(e.to_string()))?;

        stream
            .set_read_timeout(Some(Duration::from_millis(self.config.timeout_ms)))
            .ok();
        stream
            .set_write_timeout(Some(Duration::from_millis(self.config.timeout_ms)))
            .ok();

        self.connected = true;
        Ok(stream)
    }

    fn change_port(&mut self) {
        self.current_port_index = (self.current_port_index + 1) % self.config.ports.len();
    }

    fn build_request(&self, endpoint: &str) -> String {
        format!(
            "GET {} HTTP/1.1\r\n\
             Host: {}\r\n\
             Cache-Control: no-cache\r\n\
             User-Agent:AppleWebKit/537.36\r\n\r\n",
            endpoint,
            self.config.host
        )
    }

    fn send_request(&self, stream: &mut TcpStream, endpoint: &str) -> Result<(), ChannelError> {
        let request = self.build_request(endpoint);
        stream
            .write_all(request.as_bytes())
            .map_err(|e| ChannelError::WriteError(e.to_string()))
    }

    fn read_response(&self, stream: &mut TcpStream, buf: &mut [u8]) -> Result<usize, ChannelError> {
        stream
            .read(buf)
            .map_err(|e| ChannelError::ReadError(e.to_string()))
    }

    fn extract_body(response: &[u8]) -> Result<&[u8], ChannelError> {
        let response_str = String::from_utf8_lossy(response);

        let body_start = response_str
            .find("\r\n\r\n")
            .ok_or(ChannelError::InvalidResponse)?;

        let body = &response[body_start + 4..];

        let first_char = body.first().copied();
        if first_char == Some(b'{') || first_char == Some(b'[') {
            Ok(body)
        } else if first_char == Some(b'\xFF') || first_char == Some(b'M') {
            Ok(body)
        } else {
            Err(ChannelError::InvalidResponse)
        }
    }

    fn read_endpoint(&mut self, buf: &mut Vec<u8>, endpoint: &str, buf_size: usize) -> Result<(), ChannelError> {
        buf.clear();
        let mut stream = self.connect().map_err(|e| {
            self.change_port();
            self.connected = false;
            e
        })?;
        self.send_request(&mut stream, endpoint)?;
        let mut temp_buf = vec![0u8; buf_size];
        let n = self.read_response(&mut stream, &mut temp_buf)?;
        let body = Self::extract_body(&temp_buf[..n])?;
        buf.extend_from_slice(body);
        Ok(())
    }

    pub fn read_state(&mut self, buf: &mut Vec<u8>) -> Result<(), ChannelError> {
        self.read_endpoint(buf, ENDPOINT_STATE, 4096)
    }

    pub fn read_indicators(&mut self, buf: &mut Vec<u8>) -> Result<(), ChannelError> {
        self.read_endpoint(buf, ENDPOINT_INDICATORS, 8192)
    }


    pub fn read_map_info(&mut self, buf: &mut Vec<u8>) -> Result<(), ChannelError> {
        self.read_endpoint(buf, ENDPOINT_MAP_INFO, 8192)
    }


    pub fn read_map_img(&mut self, buf: &mut Vec<u8>) -> Result<(), ChannelError> {
        self.read_endpoint(buf, ENDPOINT_MAP_IMG, 1024 * 1024)
    }




}

pub mod async_channel {
    use super::*;
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;
    use tokio::time::timeout;

    const STATE_BUF_SIZE: usize = 4096;
    const INDIC_BUF_SIZE: usize = 8192;
    const MAP_INFO_BUF_SIZE: usize = 4096;
    const MAP_OBJ_BUF_SIZE: usize = 65536;

    pub struct AsyncChannel {
        config: ChannelConfig,
        current_index: Arc<std::sync::Mutex<usize>>,
        req_state: Vec<u8>,
        req_indic: Vec<u8>,
        req_map_info: Vec<u8>,
        req_map: Vec<u8>,
    }

    fn build_req(endpoint: &str, host: Ipv4Addr) -> Vec<u8> {
        format!(
            "GET {} HTTP/1.1\r\n\
             Host: {}\r\n\
             Cache-Control: no-cache\r\n\
             \r\n",
            endpoint, host
        ).into_bytes()
    }

    #[derive(Debug)]
    pub struct ChannelReadResult {
        pub state: Result<Vec<u8>, ChannelError>,
        pub indicators: Result<Vec<u8>, ChannelError>,
        pub map_objects: Result<Vec<u8>, ChannelError>,
    }

    pub struct ChannelParseResult {
        pub state: Result<(), ()>,
        pub indicators: Result<(), ()>,
        pub map_objects: Result<bool, ()>,
    }

    impl AsyncChannel {
        pub fn new(config: ChannelConfig) -> Self {
            let host = config.host;
            Self {
                config: config.clone(),
                current_index: Arc::new(std::sync::Mutex::new(0)),
                req_state: build_req(ENDPOINT_STATE, host),
                req_indic: build_req(ENDPOINT_INDICATORS, host),
                req_map_info: build_req(ENDPOINT_MAP_INFO, host),
                req_map: build_req(ENDPOINT_MAP_OBJ, host),
            }
        }


        pub fn with_ip_and_ports(host: Ipv4Addr, ports: Vec<u16>) -> Self {
            Self::new(ChannelConfig::new(host, ports))
        }

        fn get_port(&self) -> u16 {
            let idx = *self.current_index.lock().unwrap();
            self.config.ports[idx % self.config.ports.len()]
        }

        fn rotate_port(&self) {
            let mut idx = self.current_index.lock().unwrap();
            *idx = (*idx + 1) % self.config.ports.len();
        }

        async fn do_read_with(
            &self,
            req_bytes: &[u8],
            buf_size: usize,
        ) -> Result<Vec<u8>, ChannelError> {
            let port = self.get_port();
            let addr = SocketAddr::new(IpAddr::V4(self.config.host), port);

            let mut stream = match timeout(
                Duration::from_millis(self.config.timeout_ms),
                TcpStream::connect(addr),
            )
            .await
            {
                Ok(Ok(s)) => s,
                Ok(Err(e)) => {
                    self.rotate_port();
                    return Err(ChannelError::ConnectionFailed(e.to_string()));
                }
                Err(_) => {
                    self.rotate_port();
                    return Err(ChannelError::Timeout);
                }
            };

            if let Err(e) = stream.write_all(req_bytes).await {
                return Err(ChannelError::WriteError(e.to_string()));
            }

            let mut temp_buf = vec![0u8; buf_size];
            let n = match timeout(
                Duration::from_millis(self.config.timeout_ms),
                stream.read(&mut temp_buf),
            )
            .await
            {
                Ok(Ok(n)) => n,
                Ok(Err(e)) => return Err(ChannelError::ReadError(e.to_string())),
                Err(_) => return Err(ChannelError::Timeout),
            };

            let body = Self::extract_body(&temp_buf[..n])?;
            Ok(body.to_vec())
        }

        pub async fn read_state(&mut self, buf: &mut Vec<u8>) -> Result<(), ChannelError> {
            buf.clear();
            *buf = self.do_read_with(&self.req_state, STATE_BUF_SIZE).await?;
            Ok(())
        }

        pub async fn read_indicators(&mut self, buf: &mut Vec<u8>) -> Result<(), ChannelError> {
            buf.clear();
            *buf = self.do_read_with(&self.req_indic, INDIC_BUF_SIZE).await?;
            Ok(())
        }

        pub async fn read_map_info(&mut self, buf: &mut Vec<u8>) -> Result<(), ChannelError> {
            buf.clear();
            *buf = self.do_read_with(&self.req_map_info, MAP_INFO_BUF_SIZE).await?;
            Ok(())
        }

        pub async fn read_map_objects(&mut self, buf: &mut Vec<u8>) -> Result<(), ChannelError> {
            buf.clear();
            *buf = self.do_read_with(&self.req_map, MAP_OBJ_BUF_SIZE).await?;
            Ok(())
        }

        pub async fn read_all(
            &self,
            state: &mut crate::parser::FlightState,
            indic: &mut crate::parser::Indicators,
            map_obj: &mut crate::parser::MapObjData,
            read_map_objects: bool,
        ) -> Result<ChannelParseResult, ChannelError> {
            let parse_state = |bytes: Vec<u8>, state: &mut crate::parser::FlightState| -> Result<(), ()> {
                let s = String::from_utf8_lossy(&bytes).into_owned();
                match crate::parser::parse_state(state, &s) {
                    Ok(()) => Ok(()),
                    Err(e) => {
                        warn(&WARN_PARSE_STATE, &format!("[WARN] parse_state error: {e:?}"));
                        warn_bytes(&WARN_PARSE_STATE, "state", &s);
                        Err(())
                    }
                }
            };

            let parse_indicators = |bytes: Vec<u8>, indic: &mut crate::parser::Indicators| -> Result<(), ()> {
                let s = String::from_utf8_lossy(&bytes).into_owned();
                match crate::parser::parse_indicators(indic, &s) {
                    Ok(()) => Ok(()),
                    Err(e) => {
                        warn(&WARN_PARSE_INDIC, &format!("[WARN] parse_indicators error: {e:?}"));
                        warn_bytes(&WARN_PARSE_INDIC, "indicators", &s);
                        Err(())
                    }
                }
            };

            let parse_map_obj = |bytes: Vec<u8>, obj: &mut crate::parser::MapObjData| -> Result<bool, ()> {
                if bytes.is_empty() {
                    return Ok(false);
                }
                let s = String::from_utf8_lossy(&bytes).into_owned();
                match crate::parser::parse_map_obj_data(obj, &s) {
                    Ok(()) => Ok(!obj.objects.is_empty()),
                    Err(e) => {
                        warn(&WARN_PARSE_MAPOBJ, &format!("[WARN] parse_map_obj error: {e:?}"));
                        warn_bytes(&WARN_PARSE_MAPOBJ, "map_obj", &s);
                        Err(())
                    }
                }
            };

            let (state_result, indic_result, map_result) = if read_map_objects {
                let (state_bytes, indic_bytes, map_bytes) = tokio::join!(
                    self.do_read_with(&self.req_state, STATE_BUF_SIZE),
                    self.do_read_with(&self.req_indic, INDIC_BUF_SIZE),
                    self.do_read_with(&self.req_map, MAP_OBJ_BUF_SIZE),
                );

                let state = state_bytes.map(|b| parse_state(b, state)).unwrap_or_else(|e| {
                    warn(&WARN_READ_STATE, &format!("[WARN] read_state error: {e:?}"));
                    Err(())
                });
                let indicators = indic_bytes.map(|b| parse_indicators(b, indic)).unwrap_or_else(|e| {
                    warn(&WARN_READ_INDIC, &format!("[WARN] read_indicators error: {e:?}"));
                    Err(())
                });
                let map_objects = map_bytes.map(|b| parse_map_obj(b, map_obj)).unwrap_or_else(|e| {
                    warn(&WARN_READ_MAPOBJ, &format!("[WARN] read_map_objects error: {e:?}"));
                    Err(())
                });

                (state, indicators, map_objects)
            } else {
                let (state_bytes, indic_bytes) = tokio::join!(
                    self.do_read_with(&self.req_state, STATE_BUF_SIZE),
                    self.do_read_with(&self.req_indic, INDIC_BUF_SIZE),
                );

                let state = state_bytes.map(|b| parse_state(b, state)).unwrap_or_else(|e| {
                    warn(&WARN_READ_STATE, &format!("[WARN] read_state error: {e:?}"));
                    Err(())
                });
                let indicators = indic_bytes.map(|b| parse_indicators(b, indic)).unwrap_or_else(|e| {
                    warn(&WARN_READ_INDIC, &format!("[WARN] read_indicators error: {e:?}"));
                    Err(())
                });

                (state, indicators, Ok(false))
            };

            Ok(ChannelParseResult {
                state: state_result,
                indicators: indic_result,
                map_objects: map_result,
            })
        }

        fn extract_body(response: &[u8]) -> Result<&[u8], ChannelError> {
            super::Channel::extract_body(response)
        }

    }
}

/// 限流打印（站点各自的 `AtomicU64`）。
fn warn(site: &AtomicU64, msg: &str) {
    crate::warn_throttled(site, || msg.to_string());
}

/// 解析失败时打前 200 字节原文（同样限流）。
fn warn_bytes(site: &AtomicU64, what: &str, body: &str) {
    crate::warn_throttled(site, || {
        format!("[WARN] {what} bytes (first 200): {:?}", &body[..body.len().min(200)])
    });
}
