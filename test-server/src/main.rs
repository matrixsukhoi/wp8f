use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use tracing::{error, info, warn};

mod scenario;
use scenario::Scenario;

fn format_wt_value(val: &toml::Value) -> String {
    match val {
        toml::Value::String(s) => {
            let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
            format!("\"{}\"", escaped)
        }
        toml::Value::Integer(i) => i.to_string(),
        toml::Value::Float(f) => {
            if f.is_nan() || f.is_infinite() {
                return "0.0".to_string();
            }
            let s = format!("{}", f);
            if !s.contains('.') && !s.contains('e') && !s.contains('E') {
                format!("{}.0", s)
            } else {
                s
            }
        }
        toml::Value::Boolean(b) => b.to_string(),
        toml::Value::Array(arr) => {
            let items: Vec<String> = arr.iter().map(format_wt_value).collect();
            format!("[ {} ]", items.join(", "))
        }
        _ => "null".to_string(),
    }
}

fn format_wt_object(map: &HashMap<String, toml::Value>, separator: &str) -> String {
    let mut keys: Vec<&String> = map.keys().collect();
    keys.sort();
    let parts: Vec<String> = keys
        .iter()
        .map(|k| format!("\"{}\"{}{}", k, separator, format_wt_value(&map[*k])))
        .collect();
    format!("{{{}}}", parts.join(","))
}

fn format_wt_array(arr: &[HashMap<String, toml::Value>]) -> String {
    let items: Vec<String> = arr
        .iter()
        .map(|obj| format_wt_object(obj, ":"))
        .collect();
    format!("[{}]", items.join(","))
}

#[derive(Parser, Debug)]
#[command(name = "test-server")]
#[command(author = "wp8f contributors")]
#[command(version = "0.1.0")]
#[command(about = "Test server for wp8f - simulates War Thunder game server")]
struct Args {
    #[arg(short, long, default_value = "scenarios/default.toml")]
    scenario: PathBuf,

    #[arg(
        short,
        long,
        use_value_delimiter = true,
        value_delimiter = ',',
        default_value = "8111,9222"
    )]
    port: Vec<u16>,
}

fn load_scenario(path: &PathBuf) -> Result<Scenario, String> {
    let resolved = if path.is_relative() {
        std::env::current_dir()
            .map_err(|e| format!("Failed to get current dir: {}", e))?
            .join(path)
    } else {
        path.clone()
    };

    let content = fs::read_to_string(&resolved)
        .map_err(|e| format!("Failed to read file '{}': {}", resolved.display(), e))?;
    toml::from_str(&content).map_err(|e| format!("Failed to parse TOML: {}", e))
}

fn build_http_response(body: &str, content_type: &str) -> String {
    let body_bytes = body.as_bytes();
    format!(
        "HTTP/1.1 200 OK\r\n\
         Server: test-server\r\n\
         Content-Type: {}\r\n\
         Content-Length: {}\r\n\
         Access-Control-Allow-Origin: *\r\n\
         \r\n{}",
        content_type,
        body_bytes.len(),
        body
    )
}

fn build_binary_response(body: &[u8], content_type: &str) -> Vec<u8> {
    let mut response = Vec::new();
    response.extend_from_slice(b"HTTP/1.1 200 OK\r\n");
    response.extend_from_slice(b"Server: test-server\r\n");
    response.extend_from_slice(format!("Content-Type: {}\r\n", content_type).as_bytes());
    response.extend_from_slice(format!("Content-Length: {}\r\n", body.len()).as_bytes());
    response.extend_from_slice(b"Access-Control-Allow-Origin: *\r\n");
    response.extend_from_slice(b"\r\n");
    response.extend_from_slice(body);
    response
}

fn generate_placeholder_map_img(width: u32, height: u32) -> Vec<u8> {
    let mut img = image::ImageBuffer::new(width, height);

    for y in 0..height {
        for x in 0..width {
            let cx = x as f32 - width as f32 / 2.0;
            let cy = y as f32 - height as f32 / 2.0;
            let dist = (cx * cx + cy * cy).sqrt();
            let max_dist = (width.min(height) as f32) / 2.0;

            let intensity = if dist < max_dist * 0.3 {
                80u8
            } else if dist < max_dist * 0.6 {
                60u8
            } else if dist < max_dist * 0.85 {
                40u8
            } else {
                30u8
            };

            img.put_pixel(x, y, image::Rgb([intensity, intensity / 2, intensity / 4]));
        }
    }

    let mut jpg_data = Vec::new();
    let mut encoder = image::codecs::jpeg::JpegEncoder::new(&mut jpg_data);
    encoder
        .encode(&img, width, height, image::ExtendedColorType::Rgb8)
        .ok();
    jpg_data
}

enum HttpResponse {
    Text(String),
    Binary(Vec<u8>),
}

fn handle_request(url: &str, scenario: &Scenario) -> Option<HttpResponse> {
    info!("Received request: {}", url);

    if url.starts_with("/editor/fm_commands") {
        if let Some(query) = url.strip_prefix("/editor/fm_commands") {
            if query.contains("cmd=getFmProperties") {            } else if query.contains("cmd=setAlt") {
                if let Some(value_str) = query.strip_prefix("?cmd=setAlt&value=") {
                    info!("SET ALT: {}", value_str);
                }
                return Some(HttpResponse::Text(r#"{"result":"ok"}"#.to_string()));
            } else if query.contains("cmd=setVelocity") {
                if let Some(value_str) = query.strip_prefix("?cmd=setVelocity&value=") {
                    info!("SET VELOCITY: {}", value_str);
                }
                return Some(HttpResponse::Text(r#"{"result":"ok"}"#.to_string()));
            }
        }
        warn!("Unknown editor command: {}", url);
        return Some(HttpResponse::Text(
            r#"{"error":"unknown command"}"#.to_string(),
        ));
    }

    match url {
        "/state" => {
            let json = format_wt_object(&scenario.state, ": ");
            Some(HttpResponse::Text(json))
        }
        "/indicators" => {
            let json = format_wt_object(&scenario.indicators, ": ");
            Some(HttpResponse::Text(json))
        }
        "/map_info.json" => {
            let json = format_wt_object(&scenario.map_info, " : ");
            Some(HttpResponse::Text(json))
        }
        "/map_obj.json" => {
            let json = format_wt_array(&scenario.map_objects);
            Some(HttpResponse::Text(json))
        }
        "/map.img" => {
            let img_data = generate_placeholder_map_img(512, 512);
            Some(HttpResponse::Binary(img_data))
        }
        _ => {
            warn!("Unknown endpoint: {}", url);
            None
        }
    }
}

fn parse_http_request(request: &str) -> Option<String> {
    let line_endings = ['\n', '\r'];
    for delimiter in line_endings {
        if let Some(first_line) = request.split(delimiter).next() {
            let parts: Vec<&str> = first_line.split_whitespace().collect();
            if parts.len() >= 2 && parts[0] == "GET" {
                return Some(parts[1].to_string());
            }
        }
    }
    None
}

fn handle_client(mut stream: TcpStream, scenario: &Arc<Scenario>) {
    let mut buffer = [0u8; 65536];
    let mut request_buf = Vec::new();

    loop {
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => {
                request_buf.extend_from_slice(&buffer[..n]);
                if request_buf.contains(&b'\n') || request_buf.len() > 4096 {
                    break;
                }
            }
            Err(e) => {
                error!("Read error: {}", e);
                return;
            }
        }
    }

    let request_str = String::from_utf8_lossy(&request_buf);
    if let Some(url) = parse_http_request(&request_str) {
        if let Some(response) = handle_request(&url, scenario) {
            let write_result = match response {
                HttpResponse::Text(text) => {
                    let http_response = build_http_response(&text, "application/json");
                    stream.write_all(http_response.as_bytes())
                }
                HttpResponse::Binary(data) => {
                    let http_response = build_binary_response(&data, "application/octet-stream");
                    stream.write_all(&http_response)
                }
            };
            if let Err(e) = write_result {
                error!("Write error: {}", e);
            }
            let _ = stream.flush();
        }
    }
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .init();

    let args = Args::parse();

    info!("Loading scenario from: {:?}", args.scenario);
    let scenario = match load_scenario(&args.scenario) {
        Ok(s) => s,
        Err(e) => {
            error!("Failed to load scenario: {}", e);
            std::process::exit(1);
        }
    };

    let scenario = Arc::new(scenario);

    let mut last_error = None;
    for &port in &args.port {
        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        match TcpListener::bind(addr) {
            Ok(listener) => {
                info!("test-server listening on {}", addr);
                info!(
                    "Serving scenario: {} ({})",
                    scenario.name, scenario.description
                );
                info!("Endpoints:");
                info!("  http://{}/state", addr);
                info!("  http://{}/indicators", addr);
                info!("  http://{}/map_info.json", addr);
                info!("  http://{}/map_obj.json", addr);
                info!("  http://{}/map.img", addr);

                for stream in listener.incoming() {
                    match stream {
                        Ok(stream) => {
                            let scenario = Arc::clone(&scenario);
                            std::thread::spawn(move || {
                                handle_client(stream, &scenario);
                            });
                        }
                        Err(e) => {
                            error!("Connection error: {}", e);
                        }
                    }
                }
            }
            Err(e) => {
                last_error = Some((port, e));
                continue;
            }
        }
        return;
    }

    if let Some((port, err)) = last_error {
        error!("Failed to bind to port {}: {}", port, err);
    }
    error!(
        "Could not bind to any of the specified ports: {:?}",
        args.port
    );
    std::process::exit(1);
}
