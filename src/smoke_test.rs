use crate::grpc::{encode_varint, grpc_frame, protobuf_bytes, protobuf_string};
use anyhow::{anyhow, bail, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use std::env;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const DEFAULT_ORIGIN: &str = "https://app.monsgeek.com";
const METHODS: &[(&str, Payload)] = &[
    ("getVersion", Payload::Empty),
    ("watchDevList", Payload::Empty),
    ("watchVender", Payload::Empty),
    ("watchSystemInfo", Payload::Empty),
    ("sendMsg", Payload::SendMsg),
    ("readMsg", Payload::ReadMsg),
    ("sendRawFeature", Payload::SendMsg),
    ("readRawFeature", Payload::ReadMsg),
    ("setLightType", Payload::SetLightType),
    ("muteMicrophone", Payload::BoolTrue),
    ("toggleMicrophoneMute", Payload::Empty),
    ("getMicrophoneMute", Payload::Empty),
    ("changeWirelessLoopStatus", Payload::BoolFalse),
    ("insertDb", Payload::InsertDb),
    ("getItemFromDb", Payload::GetDb),
    ("getAllKeysFromDb", Payload::DbPath),
    ("getAllValuesFromDb", Payload::DbPath),
    ("deleteItemFromDb", Payload::GetDb),
    ("cleanDev", Payload::ReadMsg),
    ("getWeather", Payload::Weather),
    ("upgradeOTAGATT", Payload::Ota),
];

#[derive(Clone, Copy)]
enum Payload {
    Empty,
    ReadMsg,
    SendMsg,
    SetLightType,
    BoolTrue,
    BoolFalse,
    InsertDb,
    GetDb,
    DbPath,
    Weather,
    Ota,
}

pub async fn run() -> Result<()> {
    let host = env::var("MONSGEEK_BRIDGE_HOST").unwrap_or_else(|_| "127.0.0.1".into());
    let port = env::var("MONSGEEK_BRIDGE_PORT")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(3814);
    let origin = env::var("MONSGEEK_SMOKE_ORIGIN").unwrap_or_else(|_| DEFAULT_ORIGIN.into());
    let mut failed = false;

    for (method, payload) in METHODS {
        let response = call(
            &host,
            port,
            method,
            &payload_bytes(*payload),
            is_streaming(method),
            &origin,
        )
        .await?;
        let ok = response.http_status == 200
            && response.grpc_status == Some(0)
            && response.allow_origin.as_deref() == Some(origin.as_str());
        failed |= !ok;
        println!(
            "{} {method} http={} grpc={} cors={} bytes={}",
            if ok { "ok" } else { "FAIL" },
            response.http_status,
            response
                .grpc_status
                .map(|value| value.to_string())
                .unwrap_or_else(|| "missing".into()),
            response.allow_origin.as_deref().unwrap_or("missing"),
            response.message_bytes
        );
    }

    if failed {
        bail!("one or more smoke-test methods failed");
    }
    Ok(())
}

struct SmokeResponse {
    http_status: u16,
    grpc_status: Option<u32>,
    allow_origin: Option<String>,
    message_bytes: usize,
}

async fn call(
    host: &str,
    port: u16,
    method: &str,
    payload: &[u8],
    streaming: bool,
    origin: &str,
) -> Result<SmokeResponse> {
    let body = STANDARD.encode(grpc_frame(0, payload));
    let request = format!(
        "POST /driver.DriverGrpc/{method} HTTP/1.1\r\n\
Host: {host}:{port}\r\n\
Content-Type: application/grpc-web-text\r\n\
Content-Length: {}\r\n\
Origin: {origin}\r\n\
X-Grpc-Web: 1\r\n\
Connection: close\r\n\
\r\n\
{body}",
        body.len()
    );

    let mut stream = TcpStream::connect((host, port))
        .await
        .with_context(|| format!("connect {host}:{port}"))?;
    stream.write_all(request.as_bytes()).await?;
    let mut response = Vec::new();
    let mut buffer = [0u8; 4096];
    loop {
        let n = tokio::time::timeout(std::time::Duration::from_secs(3), stream.read(&mut buffer))
            .await
            .with_context(|| format!("{method} timed out"))??;
        if n == 0 {
            break;
        }
        response.extend_from_slice(&buffer[..n]);
        if streaming && first_message_response(&response)?.is_some() {
            return Ok(first_message_response(&response)?.expect("checked"));
        }
    }
    parse_response(&response).with_context(|| format!("parse {method} response"))
}

fn parse_response(response: &[u8]) -> Result<SmokeResponse> {
    let split = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| anyhow!("missing HTTP header terminator"))?;
    let header = String::from_utf8_lossy(&response[..split]);
    let status = header
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|value| value.parse::<u16>().ok())
        .ok_or_else(|| anyhow!("missing HTTP status"))?;
    let body = http_body_bytes(&header, &response[split + 4..]);
    let raw = STANDARD.decode(body)?;

    let mut index = 0;
    let mut first_message = None;
    let mut grpc_status = None;
    while index + 5 <= raw.len() {
        let flag = raw[index];
        let len = u32::from_be_bytes([
            raw[index + 1],
            raw[index + 2],
            raw[index + 3],
            raw[index + 4],
        ]) as usize;
        index += 5;
        let Some(payload) = raw.get(index..index + len) else {
            break;
        };
        index += len;
        if flag == 0 && first_message.is_none() {
            first_message = Some(payload.len());
        } else if flag == 0x80 {
            grpc_status = parse_grpc_status(payload);
        }
    }

    Ok(SmokeResponse {
        http_status: status,
        grpc_status,
        allow_origin: header_value(&header, "access-control-allow-origin"),
        message_bytes: first_message.unwrap_or(0),
    })
}

fn first_message_response(response: &[u8]) -> Result<Option<SmokeResponse>> {
    let Some(split) = response.windows(4).position(|window| window == b"\r\n\r\n") else {
        return Ok(None);
    };
    let header = String::from_utf8_lossy(&response[..split]);
    let status = header
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|value| value.parse::<u16>().ok())
        .ok_or_else(|| anyhow!("missing HTTP status"))?;
    let body = http_body_bytes(&header, &response[split + 4..]);
    let Ok(raw) = STANDARD.decode(body) else {
        return Ok(None);
    };
    if raw.len() < 5 || raw[0] != 0 {
        return Ok(None);
    }
    let len = u32::from_be_bytes([raw[1], raw[2], raw[3], raw[4]]) as usize;
    if raw.len() < 5 + len {
        return Ok(None);
    }
    Ok(Some(SmokeResponse {
        http_status: status,
        grpc_status: Some(0),
        allow_origin: header_value(&header, "access-control-allow-origin"),
        message_bytes: len,
    }))
}

fn header_value(header: &str, name: &str) -> Option<String> {
    header.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.eq_ignore_ascii_case(name)
            .then(|| value.trim().to_string())
    })
}

fn http_body_bytes(header: &str, body: &[u8]) -> Vec<u8> {
    if header
        .lines()
        .any(|line| line.eq_ignore_ascii_case("transfer-encoding: chunked"))
    {
        decode_available_chunks(body)
    } else {
        body.to_vec()
    }
}

fn decode_available_chunks(mut body: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let Some(line_end) = body.windows(2).position(|window| window == b"\r\n") else {
            break;
        };
        let size_text = String::from_utf8_lossy(&body[..line_end]);
        let size_text = size_text.split(';').next().unwrap_or("").trim();
        let Ok(size) = usize::from_str_radix(size_text, 16) else {
            break;
        };
        body = &body[line_end + 2..];
        if size == 0 {
            break;
        }
        if body.len() < size + 2 {
            break;
        }
        out.extend_from_slice(&body[..size]);
        body = &body[size + 2..];
    }
    out
}

fn parse_grpc_status(trailer: &[u8]) -> Option<u32> {
    String::from_utf8_lossy(trailer)
        .lines()
        .find_map(|line| line.strip_prefix("grpc-status:"))
        .and_then(|value| value.trim().parse().ok())
}

fn payload_bytes(payload: Payload) -> Vec<u8> {
    let hidraw = env::var("MONSGEEK_HIDRAW").unwrap_or_else(|_| "/dev/hidraw4".into());
    let read_msg = protobuf_string(1, &hidraw);
    let db_path = "web_driver/iot_db/smoke";
    let db_key = b"smoke-key";
    let db_value = b"smoke-value";
    match payload {
        Payload::Empty => Vec::new(),
        Payload::ReadMsg => read_msg,
        Payload::SendMsg => [
            read_msg,
            protobuf_bytes(2, &[0x8f]),
            protobuf_bool(3, false),
        ]
        .concat(),
        Payload::SetLightType => [protobuf_string(1, &hidraw), protobuf_varint_raw(2, 0)].concat(),
        Payload::BoolTrue => protobuf_bool(1, true),
        Payload::BoolFalse => protobuf_bool(1, false),
        Payload::InsertDb => [
            protobuf_string(1, db_path),
            protobuf_bytes(2, db_key),
            protobuf_bytes(3, db_value),
        ]
        .concat(),
        Payload::GetDb => [protobuf_string(1, db_path), protobuf_bytes(2, db_key)].concat(),
        Payload::DbPath => protobuf_string(1, db_path),
        Payload::Weather => [protobuf_string(1, "en"), protobuf_string(2, "Macau")].concat(),
        Payload::Ota => [
            protobuf_string(1, &hidraw),
            protobuf_bytes(2, &[1, 2, 3, 4]),
        ]
        .concat(),
    }
}

fn is_streaming(method: &str) -> bool {
    matches!(method, "watchDevList" | "watchVender" | "watchSystemInfo")
}

fn protobuf_bool(field: u32, value: bool) -> Vec<u8> {
    protobuf_varint_raw(field, u64::from(value))
}

fn protobuf_varint_raw(field: u32, value: u64) -> Vec<u8> {
    [encode_varint((field as u64) << 3), encode_varint(value)].concat()
}
