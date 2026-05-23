use anyhow::{anyhow, Context, Result};
use axum::{
    body::{Body, Bytes},
    extract::Path,
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::options,
    Router,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use percent_encoding::{percent_encode, NON_ALPHANUMERIC};
use serde_json::{json, Map, Value};
use std::{
    collections::HashMap,
    env,
    ffi::CString,
    fs,
    io::ErrorKind,
    os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{
    net::TcpListener,
    sync::{broadcast, Mutex},
    task::JoinHandle,
};
use tokio_stream::{wrappers::BroadcastStream, StreamExt};

const HOST: &str = "127.0.0.1";
const PORT: u16 = 3814;
const WEB_ORIGIN: &str = "https://web.monsgeek.com";
const HIDRAW_SYSFS: &str = "/sys/class/hidraw";
const MONSGEEK_VENDOR: &str = "3151";
const MONSGEEK_PRODUCT: &str = "502d";
const GET_INFOR: u8 = 0x8f;
const CHECKSUM_BIT7: u32 = 0;
const CHECKSUM_BIT8: u32 = 1;
const LIGHT_OTHER: u32 = 2;
const FEA_CMD_SET_MAGNETISM_REPORT: u8 = 0x1b;
const FEA_CMD_SET_MAGNETISM_CAL: u8 = 0x1c;
const FEA_CMD_SET_MAGNETISM_CALMAX: u8 = 0x1e;
const FEA_CMD_GET_MAGNETISM_BY_ARR: u8 = 0xe5;
const MAGNETISM_TRAVEL_VALUES: u8 = 0xfe;
const CALIBRATION_KEYS_PER_PAGE: usize = 32;
const CALIBRATION_MAX_KEYS: usize = CALIBRATION_KEYS_PER_PAGE * 4;
const REPORT_BYTES: usize = 64;

#[derive(Clone)]
struct Config {
    default_hidraw: String,
    vendor_hidraws: Vec<String>,
    db_file: PathBuf,
    trace_hid: bool,
    trace_focus: bool,
    vendor_input_reader: bool,
    calibration_input_cache: bool,
    calibration_input_cache_ttl: Duration,
    calibration_physical_input_grace: Duration,
    calibration_input_stabilize: Duration,
    calibration_input_confirm: Duration,
    calibration_press_select: Duration,
    mac_send_settle: Duration,
    mac_read_poll: Duration,
    mac_read_attempts: usize,
    allow_ota: bool,
}

#[derive(Clone, Default)]
#[allow(dead_code)]
struct LightState {
    light_type: u32,
    screen_id: u32,
    dangle_dev_type: u32,
}

struct PressState {
    active: bool,
    key_index: Option<usize>,
    started_at: Instant,
    candidates: HashMap<usize, Candidate>,
}

impl Default for PressState {
    fn default() -> Self {
        Self {
            active: false,
            key_index: None,
            started_at: Instant::now(),
            candidates: HashMap::new(),
        }
    }
}

#[derive(Clone, Copy)]
struct Candidate {
    count: u32,
    max: u16,
    last_at: Option<Instant>,
}

impl Default for Candidate {
    fn default() -> Self {
        Self {
            count: 0,
            max: 0,
            last_at: None,
        }
    }
}

struct CalibrationState {
    active: bool,
    maximum: bool,
    travel_reads: u64,
    input_max: [u16; CALIBRATION_MAX_KEYS],
    input_active_until: Option<Instant>,
    input_ignore_until: Option<Instant>,
    pending_input: Option<PendingInput>,
    press: PressState,
    device_path: String,
}

impl CalibrationState {
    fn new(device_path: String) -> Self {
        Self {
            active: false,
            maximum: false,
            travel_reads: 0,
            input_max: [0; CALIBRATION_MAX_KEYS],
            input_active_until: None,
            input_ignore_until: None,
            pending_input: None,
            press: PressState::default(),
            device_path,
        }
    }
}

#[derive(Clone, Copy)]
struct PendingInput {
    key_index: usize,
    travel: u16,
    time: Instant,
    count: u32,
}

#[derive(Default)]
struct MagnetismReportState {
    active: bool,
}

struct DeviceState {
    last_command: Option<u8>,
    light: LightState,
    calibration: CalibrationState,
    magnetism_report: MagnetismReportState,
    last_magnetism_read: Option<MagnetismRead>,
}

impl DeviceState {
    fn new(device_path: String) -> Self {
        Self {
            last_command: None,
            light: LightState {
                light_type: LIGHT_OTHER,
                ..LightState::default()
            },
            calibration: CalibrationState::new(device_path),
            magnetism_report: MagnetismReportState::default(),
            last_magnetism_read: None,
        }
    }
}

#[derive(Clone, Copy)]
struct MagnetismRead {
    kind: u8,
    page: usize,
}

struct AppState {
    config: Config,
    devices: Mutex<HashMap<String, DeviceState>>,
    vendor_readers: Mutex<HashMap<String, JoinHandle<()>>>,
    vendor_events: broadcast::Sender<Vec<u8>>,
    microphone_muted: Mutex<bool>,
    physical_keyboard_input_active_until: Mutex<Option<Instant>>,
    last_boot_keyboard_input: Mutex<Vec<u8>>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let default_hidraw = detected_hidraw();
    let vendor_hidraws = all_monsgeek_hidraws(&default_hidraw);
    let (vendor_events, _) = broadcast::channel(256);
    let db_file = env::var("MONSGEEK_DB_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let state_home = env::var("XDG_STATE_HOME").unwrap_or_else(|_| {
                let home = env::var("HOME").unwrap_or_else(|_| ".".into());
                format!("{home}/.local/state")
            });
            PathBuf::from(state_home).join("monsgeek-linux-bridge/db.json")
        });

    let app = Arc::new(AppState {
        config: Config {
            default_hidraw,
            vendor_hidraws,
            db_file,
            trace_hid: env_flag("MONSGEEK_TRACE_HID"),
            trace_focus: env_flag("MONSGEEK_TRACE_FOCUS"),
            vendor_input_reader: env::var("MONSGEEK_VENDOR_INPUT_READER")
                .unwrap_or_else(|_| "1".into())
                != "0",
            calibration_input_cache: env::var("MONSGEEK_CALIBRATION_INPUT_CACHE")
                .unwrap_or_else(|_| "1".into())
                != "0",
            calibration_input_cache_ttl: env_duration(
                "MONSGEEK_CALIBRATION_INPUT_CACHE_TTL_MS",
                2500,
            ),
            calibration_physical_input_grace: env_duration(
                "MONSGEEK_CALIBRATION_PHYSICAL_INPUT_GRACE_MS",
                700,
            ),
            calibration_input_stabilize: env_duration(
                "MONSGEEK_CALIBRATION_INPUT_STABILIZE_MS",
                180,
            ),
            calibration_input_confirm: env_duration("MONSGEEK_CALIBRATION_INPUT_CONFIRM_MS", 90),
            calibration_press_select: env_duration("MONSGEEK_CALIBRATION_PRESS_SELECT_MS", 45),
            mac_send_settle: env_duration("MONSGEEK_MAC_SEND_SETTLE_MS", 0),
            mac_read_poll: env_duration("MONSGEEK_MAC_READ_POLL_MS", 0),
            mac_read_attempts: env_usize("MONSGEEK_MAC_READ_POLL_ATTEMPTS", 1),
            allow_ota: env_flag("MONSGEEK_ALLOW_OTA"),
        },
        devices: Mutex::new(HashMap::new()),
        vendor_readers: Mutex::new(HashMap::new()),
        vendor_events,
        microphone_muted: Mutex::new(false),
        physical_keyboard_input_active_until: Mutex::new(None),
        last_boot_keyboard_input: Mutex::new(Vec::new()),
    });

    println!("MonsGeek Rust bridge listening on http://{HOST}:{PORT}");
    println!("Using HID endpoint {}", app.config.default_hidraw);
    println!("Using DB file {}", app.config.db_file.display());

    let router = Router::new()
        .route(
            "/driver.DriverGrpc/{method}",
            options(options_handler).post(post_handler),
        )
        .fallback(options(options_handler).post(post_handler))
        .with_state(app);

    let listener = TcpListener::bind((HOST, PORT)).await?;
    axum::serve(listener, router).await?;
    Ok(())
}

async fn options_handler() -> impl IntoResponse {
    (StatusCode::NO_CONTENT, headers("text/plain"), "")
}

async fn post_handler(
    axum::extract::State(app): axum::extract::State<Arc<AppState>>,
    Path(method): Path<String>,
    body: Bytes,
) -> Response {
    let request = decode_grpc_text_body(&body);
    let result = match method.as_str() {
        "watchDevList" => Ok(ResponseResult::ok(
            device_list_message(&app).await.unwrap_or_default(),
            "stream",
        )),
        "watchVender" => {
            start_vendor_input_readers(app.clone()).await;
            return vendor_stream_response(app).await;
        }
        "watchSystemInfo" => Ok(ResponseResult::ok(system_info_message(), "stream")),
        _ => response_for(app, method.as_str(), &request).await,
    }
    .unwrap_or_else(|error| ResponseResult {
        message: Vec::new(),
        status: 13,
        status_message: error.to_string(),
        note: format!("request failed: {error}"),
    });

    println!("POST /driver.DriverGrpc/{method} -> {}", result.note);
    (
        StatusCode::OK,
        headers("application/grpc-web-text"),
        grpc_text_response(&result.message, result.status, &result.status_message),
    )
        .into_response()
}

async fn vendor_stream_response(app: Arc<AppState>) -> Response {
    println!("POST /driver.DriverGrpc/watchVender -> vendor event stream");
    let receiver = app.vendor_events.subscribe();
    let initial = tokio_stream::once(Ok::<Bytes, std::convert::Infallible>(Bytes::from(
        STANDARD.encode(grpc_frame(0, &vender_message(&[]))),
    )));
    let events = BroadcastStream::new(receiver).filter_map(|message| match message {
        Ok(payload) => Some(Ok::<Bytes, std::convert::Infallible>(Bytes::from(
            STANDARD.encode(grpc_frame(0, &vender_message(&payload))),
        ))),
        Err(_) => None,
    });
    (
        StatusCode::OK,
        headers("application/grpc-web-text"),
        Body::from_stream(initial.chain(events)),
    )
        .into_response()
}

struct ResponseResult {
    message: Vec<u8>,
    status: u32,
    status_message: String,
    note: String,
}

impl ResponseResult {
    fn ok(message: Vec<u8>, note: impl Into<String>) -> Self {
        Self {
            message,
            status: 0,
            status_message: String::new(),
            note: note.into(),
        }
    }
}

async fn response_for(app: Arc<AppState>, method: &str, request: &[u8]) -> Result<ResponseResult> {
    match method {
        "getVersion" => Ok(ResponseResult::ok(
            [
                protobuf_string(1, "1.8.9-linux-rust"),
                protobuf_string(2, "20260523"),
            ]
            .concat(),
            "version response",
        )),
        "changeWirelessLoopStatus" | "cleanDev" => {
            Ok(ResponseResult::ok(Vec::new(), "acknowledged"))
        }
        "setLightType" => {
            let light = decode_set_light(request, &app.config.default_hidraw)?;
            let mut devices = app.devices.lock().await;
            let state = devices
                .entry(light.device_path.clone())
                .or_insert_with(|| DeviceState::new(light.device_path.clone()));
            state.light = LightState {
                light_type: light.light_type,
                screen_id: light.screen_id,
                dangle_dev_type: light.dangle_dev_type,
            };
            let _ = app.vendor_events.send(vender_light_payload(&light));
            Ok(ResponseResult::ok(Vec::new(), "setLightType"))
        }
        "muteMicrophone" => {
            *app.microphone_muted.lock().await = decode_bool_field(request, 1, false)?;
            Ok(ResponseResult::ok(Vec::new(), "microphone mute set"))
        }
        "toggleMicrophoneMute" => {
            let mut muted = app.microphone_muted.lock().await;
            *muted = !*muted;
            Ok(ResponseResult::ok(Vec::new(), "microphone mute toggled"))
        }
        "getMicrophoneMute" => {
            let muted = *app.microphone_muted.lock().await;
            Ok(ResponseResult::ok(
                protobuf_varint(1, muted as u64),
                "microphone mute status",
            ))
        }
        "getWeather" => Ok(ResponseResult::ok(
            weather_response(request)?,
            "synthetic weather response",
        )),
        "upgradeOTAGATT" => {
            let req = decode_ota_upgrade(request, &app.config.default_hidraw)?;
            let err = if app.config.allow_ota {
                "Linux OTA transport is not implemented yet"
            } else {
                "Linux OTA is disabled; set MONSGEEK_ALLOW_OTA=1 only after implementing device-specific flashing"
            };
            Ok(ResponseResult::ok(
                ota_progress(err, 0.0),
                format!(
                    "OTA refused for {} bytes={}",
                    req.device_path, req.file_bytes
                ),
            ))
        }
        "insertDb" => {
            let req = decode_db_message(request)?;
            store_db_item(&app.config.db_file, &req)?;
            Ok(ResponseResult::ok(Vec::new(), "DB put"))
        }
        "getItemFromDb" => {
            let req = decode_db_message(request)?;
            Ok(ResponseResult::ok(
                stored_db_item(&app.config.db_file, &req)?,
                "DB get",
            ))
        }
        "getAllKeysFromDb" => {
            let req = decode_db_message(request)?;
            Ok(ResponseResult::ok(
                stored_db_list(&app.config.db_file, &req.dbpath, true)?,
                "DB keys",
            ))
        }
        "getAllValuesFromDb" => {
            let req = decode_db_message(request)?;
            Ok(ResponseResult::ok(
                stored_db_list(&app.config.db_file, &req.dbpath, false)?,
                "DB values",
            ))
        }
        "deleteItemFromDb" => {
            let req = decode_db_message(request)?;
            delete_db_item(&app.config.db_file, &req)?;
            Ok(ResponseResult::ok(Vec::new(), "DB delete"))
        }
        "sendMsg" | "sendRawFeature" => {
            let message = decode_send_msg(request, &app.config.default_hidraw)?;
            let payload = normalized_payload(&message.payload, message.checksum_type);
            let send_result = mac_like_send_feature(&app, &message.device_path, &payload).await;
            update_write_state(&app, &message.device_path, &payload).await;
            Ok(ResponseResult::ok(
                res_send(
                    send_result
                        .err()
                        .map(|e| e.to_string())
                        .as_deref()
                        .unwrap_or(""),
                ),
                "sendMsg",
            ))
        }
        "readMsg" | "readRawFeature" => {
            let message = decode_read_msg(request, &app.config.default_hidraw)?;
            let mut raw = match mac_like_read_feature(&app, &message.device_path).await {
                Ok(payload) => payload,
                Err(error) => {
                    return Ok(ResponseResult::ok(
                        res_read(&[], &error.to_string()),
                        "readMsg failed",
                    ))
                }
            };
            let mut devices = app.devices.lock().await;
            let state = devices
                .entry(message.device_path.clone())
                .or_insert_with(|| DeviceState::new(message.device_path.clone()));
            raw = monotonic_calibration_payload(&app, state, &raw);
            raw = cached_calibration_input_payload(&app, state, &raw);
            Ok(ResponseResult::ok(res_read(&raw, ""), "readMsg"))
        }
        _ => Ok(ResponseResult {
            message: Vec::new(),
            status: 12,
            status_message: "linux bridge does not implement this method yet".into(),
            note: "unimplemented method".into(),
        }),
    }
}

fn env_flag(name: &str) -> bool {
    env::var(name).map(|value| value == "1").unwrap_or(false)
}

fn env_duration(name: &str, default_ms: u64) -> Duration {
    Duration::from_millis(
        env::var(name)
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(default_ms),
    )
}

fn env_usize(name: &str, default: usize) -> usize {
    env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn detected_hidraw() -> String {
    if let Ok(path) = env::var("MONSGEEK_HIDRAW") {
        return path;
    }
    if let Ok(entries) = fs::read_dir(HIDRAW_SYSFS) {
        let mut names = entries
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| name.starts_with("hidraw"))
            .collect::<Vec<_>>();
        names.sort();
        for name in names {
            let sysfs_device = PathBuf::from(HIDRAW_SYSFS).join(&name).join("device");
            let Ok(real) = fs::canonicalize(&sysfs_device) else {
                continue;
            };
            let Some((vendor, product)) = usb_parent(&real) else {
                continue;
            };
            let descriptor = fs::read(sysfs_device.join("report_descriptor")).unwrap_or_default();
            if vendor == MONSGEEK_VENDOR
                && product == MONSGEEK_PRODUCT
                && is_vendor_feature_interface(&descriptor)
            {
                return format!("/dev/{name}");
            }
        }
    }
    "/dev/hidraw4".into()
}

fn all_monsgeek_hidraws(default_hidraw: &str) -> Vec<String> {
    if let Ok(paths) = env::var("MONSGEEK_VENDOR_HIDRAWS") {
        return paths
            .split(',')
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .map(str::to_string)
            .collect();
    }
    let mut paths = Vec::new();
    if let Ok(entries) = fs::read_dir(HIDRAW_SYSFS) {
        let mut names = entries
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| name.starts_with("hidraw"))
            .collect::<Vec<_>>();
        names.sort();
        for name in names {
            let sysfs_device = PathBuf::from(HIDRAW_SYSFS).join(&name).join("device");
            let Ok(real) = fs::canonicalize(&sysfs_device) else {
                continue;
            };
            if usb_parent(&real).is_some_and(|(vendor, product)| {
                vendor == MONSGEEK_VENDOR && product == MONSGEEK_PRODUCT
            }) {
                paths.push(format!("/dev/{name}"));
            }
        }
    }
    if !paths.iter().any(|path| path == default_hidraw) {
        paths.push(default_hidraw.to_string());
    }
    paths
}

fn usb_parent(start: &std::path::Path) -> Option<(String, String)> {
    let mut current = start.to_path_buf();
    loop {
        let vendor = fs::read_to_string(current.join("idVendor"))
            .ok()
            .map(|v| v.trim().to_ascii_lowercase());
        let product = fs::read_to_string(current.join("idProduct"))
            .ok()
            .map(|v| v.trim().to_ascii_lowercase());
        if let (Some(vendor), Some(product)) = (vendor, product) {
            return Some((vendor, product));
        }
        if !current.pop() || current == PathBuf::from("/sys") {
            return None;
        }
    }
}

fn is_vendor_feature_interface(descriptor: &[u8]) -> bool {
    let text = hex(descriptor);
    text.contains("06ffff") && text.contains("9540") && text.contains("7508") && text.contains("b1")
}

fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn headers(content_type: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        "access-control-allow-credentials",
        HeaderValue::from_static("true"),
    );
    headers.insert(
        "access-control-allow-headers",
        HeaderValue::from_static("content-type,x-grpc-web,x-user-agent,grpc-timeout,authorization"),
    );
    headers.insert(
        "access-control-allow-methods",
        HeaderValue::from_static("POST,OPTIONS"),
    );
    headers.insert(
        "access-control-allow-private-network",
        HeaderValue::from_static("true"),
    );
    headers.insert(
        "access-control-allow-origin",
        HeaderValue::from_static(WEB_ORIGIN),
    );
    headers.insert(
        "access-control-expose-headers",
        HeaderValue::from_static("grpc-status,grpc-message,grpc-status-details-bin"),
    );
    headers.insert("content-type", HeaderValue::from_str(content_type).unwrap());
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    headers
}

fn encode_varint(mut value: u64) -> Vec<u8> {
    let mut out = Vec::new();
    while value > 0x7f {
        out.push(((value & 0x7f) as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
    out
}

fn protobuf_string(field: u32, value: &str) -> Vec<u8> {
    protobuf_bytes(field, value.as_bytes())
}

fn protobuf_varint(field: u32, value: u64) -> Vec<u8> {
    [encode_varint((field as u64) << 3), encode_varint(value)].concat()
}

fn protobuf_bytes(field: u32, value: &[u8]) -> Vec<u8> {
    [
        encode_varint(((field as u64) << 3) | 2),
        encode_varint(value.len() as u64),
        value.to_vec(),
    ]
    .concat()
}

fn protobuf_float(field: u32, value: f32) -> Vec<u8> {
    [
        encode_varint(((field as u64) << 3) | 5),
        value.to_le_bytes().to_vec(),
    ]
    .concat()
}

fn grpc_frame(flag: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = vec![flag, 0, 0, 0, 0];
    out[1..5].copy_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(payload);
    out
}

fn grpc_text_response(message: &[u8], status: u32, status_message: &str) -> String {
    let trailer = format!(
        "grpc-status:{status}\r\ngrpc-message:{}\r\n",
        percent_encode(status_message.as_bytes(), NON_ALPHANUMERIC)
    );
    STANDARD.encode([grpc_frame(0, message), grpc_frame(0x80, trailer.as_bytes())].concat())
}

fn decode_grpc_text_body(body: &[u8]) -> Vec<u8> {
    let Ok(raw) = STANDARD.decode(body) else {
        return Vec::new();
    };
    if raw.len() < 5 || raw[0] != 0 {
        return Vec::new();
    }
    let len = u32::from_be_bytes([raw[1], raw[2], raw[3], raw[4]]) as usize;
    raw.get(5..5 + len).unwrap_or_default().to_vec()
}

fn decode_varint(bytes: &[u8], index: &mut usize) -> Result<u64> {
    let mut value = 0u64;
    let mut shift = 0;
    while *index < bytes.len() {
        let byte = bytes[*index];
        *index += 1;
        value |= ((byte & 0x7f) as u64) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
        shift += 7;
    }
    Err(anyhow!("truncated protobuf varint"))
}

fn decode_len(bytes: &[u8], index: &mut usize) -> Result<Vec<u8>> {
    let len = decode_varint(bytes, index)? as usize;
    let end = *index + len;
    if end > bytes.len() {
        return Err(anyhow!("truncated protobuf bytes"));
    }
    let out = bytes[*index..end].to_vec();
    *index = end;
    Ok(out)
}

fn skip_field(bytes: &[u8], index: &mut usize, wire_type: u64) -> Result<()> {
    match wire_type {
        0 => {
            let _ = decode_varint(bytes, index)?;
            Ok(())
        }
        2 => {
            let _ = decode_len(bytes, index)?;
            Ok(())
        }
        5 => {
            *index = (*index + 4).min(bytes.len());
            Ok(())
        }
        _ => Err(anyhow!("unsupported protobuf wire type {wire_type}")),
    }
}

struct SendMsg {
    device_path: String,
    payload: Vec<u8>,
    checksum_type: u32,
    dangle_dev_type: u32,
}

fn decode_send_msg(bytes: &[u8], default_hidraw: &str) -> Result<SendMsg> {
    let mut msg = SendMsg {
        device_path: default_hidraw.into(),
        payload: Vec::new(),
        checksum_type: CHECKSUM_BIT7,
        dangle_dev_type: 0,
    };
    let mut index = 0;
    while index < bytes.len() {
        let key = decode_varint(bytes, &mut index)?;
        let field = key >> 3;
        let wire = key & 7;
        match (field, wire) {
            (1, 2) => {
                msg.device_path =
                    String::from_utf8_lossy(&decode_len(bytes, &mut index)?).into_owned()
            }
            (2, 2) => msg.payload = decode_len(bytes, &mut index)?,
            (3, 0) => msg.checksum_type = decode_varint(bytes, &mut index)? as u32,
            (4, 0) => msg.dangle_dev_type = decode_varint(bytes, &mut index)? as u32,
            _ => skip_field(bytes, &mut index, wire)?,
        }
    }
    Ok(msg)
}

struct ReadMsg {
    device_path: String,
}

fn decode_read_msg(bytes: &[u8], default_hidraw: &str) -> Result<ReadMsg> {
    let mut device_path = default_hidraw.to_string();
    let mut index = 0;
    while index < bytes.len() {
        let key = decode_varint(bytes, &mut index)?;
        let field = key >> 3;
        let wire = key & 7;
        if field == 1 && wire == 2 {
            device_path = String::from_utf8_lossy(&decode_len(bytes, &mut index)?).into_owned();
        } else {
            skip_field(bytes, &mut index, wire)?;
        }
    }
    Ok(ReadMsg { device_path })
}

struct SetLight {
    device_path: String,
    light_type: u32,
    screen_id: u32,
    dangle_dev_type: u32,
}

fn decode_set_light(bytes: &[u8], default_hidraw: &str) -> Result<SetLight> {
    let mut light = SetLight {
        device_path: default_hidraw.into(),
        light_type: LIGHT_OTHER,
        screen_id: 0,
        dangle_dev_type: 0,
    };
    let mut index = 0;
    while index < bytes.len() {
        let key = decode_varint(bytes, &mut index)?;
        let field = key >> 3;
        let wire = key & 7;
        match (field, wire) {
            (1, 2) => {
                light.device_path =
                    String::from_utf8_lossy(&decode_len(bytes, &mut index)?).into_owned()
            }
            (2, 0) => light.light_type = decode_varint(bytes, &mut index)? as u32,
            (3, 0) => light.screen_id = decode_varint(bytes, &mut index)? as u32,
            (4, 0) => light.dangle_dev_type = decode_varint(bytes, &mut index)? as u32,
            _ => skip_field(bytes, &mut index, wire)?,
        }
    }
    Ok(light)
}

fn decode_bool_field(bytes: &[u8], target: u64, default: bool) -> Result<bool> {
    let mut index = 0;
    while index < bytes.len() {
        let key = decode_varint(bytes, &mut index)?;
        let field = key >> 3;
        let wire = key & 7;
        if field == target && wire == 0 {
            return Ok(decode_varint(bytes, &mut index)? != 0);
        }
        skip_field(bytes, &mut index, wire)?;
    }
    Ok(default)
}

struct DbMessage {
    dbpath: String,
    key: Vec<u8>,
    value: Vec<u8>,
}

fn decode_db_message(bytes: &[u8]) -> Result<DbMessage> {
    let mut msg = DbMessage {
        dbpath: String::new(),
        key: Vec::new(),
        value: Vec::new(),
    };
    let mut index = 0;
    while index < bytes.len() {
        let key = decode_varint(bytes, &mut index)?;
        let field = key >> 3;
        let wire = key & 7;
        if wire == 2 && (1..=3).contains(&field) {
            let value = decode_len(bytes, &mut index)?;
            match field {
                1 => msg.dbpath = String::from_utf8_lossy(&value).into_owned(),
                2 => msg.key = value,
                3 => msg.value = value,
                _ => {}
            }
        } else {
            skip_field(bytes, &mut index, wire)?;
        }
    }
    Ok(msg)
}

struct OtaUpgrade {
    device_path: String,
    file_bytes: usize,
}

struct WeatherReq {
    language: String,
    address: String,
}

fn decode_weather_req(bytes: &[u8]) -> Result<WeatherReq> {
    let mut req = WeatherReq {
        language: "en".into(),
        address: String::new(),
    };
    let mut index = 0;
    while index < bytes.len() {
        let key = decode_varint(bytes, &mut index)?;
        let field = key >> 3;
        let wire = key & 7;
        if wire == 2 {
            let value = String::from_utf8_lossy(&decode_len(bytes, &mut index)?).into_owned();
            match field {
                1 => req.language = value,
                2 => req.address = value,
                _ => {}
            }
        } else {
            skip_field(bytes, &mut index, wire)?;
        }
    }
    Ok(req)
}

fn decode_ota_upgrade(bytes: &[u8], default_hidraw: &str) -> Result<OtaUpgrade> {
    let mut req = OtaUpgrade {
        device_path: default_hidraw.into(),
        file_bytes: 0,
    };
    let mut index = 0;
    while index < bytes.len() {
        let key = decode_varint(bytes, &mut index)?;
        let field = key >> 3;
        let wire = key & 7;
        if field == 1 && wire == 2 {
            req.device_path = String::from_utf8_lossy(&decode_len(bytes, &mut index)?).into_owned();
        } else if field == 2 && wire == 2 {
            req.file_bytes = decode_len(bytes, &mut index)?.len();
        } else {
            skip_field(bytes, &mut index, wire)?;
        }
    }
    Ok(req)
}

fn normalized_payload(payload: &[u8], checksum_type: u32) -> [u8; REPORT_BYTES] {
    let mut report = [0u8; REPORT_BYTES];
    let len = payload.len().min(REPORT_BYTES);
    report[..len].copy_from_slice(&payload[..len]);
    if checksum_type == CHECKSUM_BIT7 {
        let sum = report[..7]
            .iter()
            .fold(0u8, |acc, value| acc.wrapping_add(*value));
        report[7] = 0xffu8.wrapping_sub(sum);
    } else if checksum_type == CHECKSUM_BIT8 {
        let sum = report[..8]
            .iter()
            .fold(0u8, |acc, value| acc.wrapping_add(*value));
        report[8] = 0xffu8.wrapping_sub(sum);
    }
    report
}

async fn device_list_message(app: &Arc<AppState>) -> Result<Vec<u8>> {
    let device_id = detected_device_id(&app.config).await.unwrap_or(0);
    let mut device = Vec::new();
    device.extend(protobuf_string(3, &app.config.default_hidraw));
    device.extend(protobuf_varint(4, device_id as u64));
    device.extend(protobuf_varint(6, 1));
    device.extend(protobuf_varint(
        7,
        u64::from_str_radix(MONSGEEK_VENDOR, 16).unwrap_or(0),
    ));
    device.extend(protobuf_varint(
        8,
        u64::from_str_radix(MONSGEEK_PRODUCT, 16).unwrap_or(0),
    ));
    Ok(protobuf_bytes(1, &protobuf_bytes(1, &device)))
}

async fn detected_device_id(config: &Config) -> Result<u32> {
    if let Some(id) = env_u32("MONSGEEK_DEVICE_ID") {
        return Ok(id);
    }
    let request = normalized_payload(&[GET_INFOR], CHECKSUM_BIT7);
    let path = config.default_hidraw.clone();
    tokio::task::spawn_blocking(move || -> Result<u32> {
        hid_send_feature(&path, &request)?;
        let response = hid_get_feature(&path)?;
        if response.len() >= 3 && response[0] == GET_INFOR {
            Ok(u16::from_le_bytes([response[1], response[2]]) as u32)
        } else {
            Err(anyhow!(
                "GET_INFOR returned unexpected payload {}",
                hex(&response)
            ))
        }
    })
    .await?
}

fn env_u32(name: &str) -> Option<u32> {
    let value = env::var(name).ok()?;
    if let Some(hex_value) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        u32::from_str_radix(hex_value, 16).ok()
    } else {
        value.parse().ok()
    }
}

fn system_info_message() -> Vec<u8> {
    let (total_memory, free_memory) = memory_info_kb();
    let (total_disk, free_disk) = disk_info_bytes("/");
    let cpu_load = fs::read_to_string("/proc/loadavg")
        .ok()
        .and_then(|text| {
            text.split_whitespace()
                .next()
                .and_then(|value| value.parse::<f32>().ok())
        })
        .unwrap_or(0.0);
    let mut out = Vec::new();
    out.extend(protobuf_varint(1, total_memory));
    out.extend(protobuf_varint(2, free_memory));
    out.extend(protobuf_varint(3, total_disk));
    out.extend(protobuf_varint(4, free_disk));
    out.extend(protobuf_float(5, cpu_load));
    out
}

fn memory_info_kb() -> (u64, u64) {
    let mut total = 0;
    let mut available = 0;
    if let Ok(text) = fs::read_to_string("/proc/meminfo") {
        for line in text.lines() {
            if let Some(value) = line.strip_prefix("MemTotal:") {
                total = value
                    .split_whitespace()
                    .next()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0);
            } else if let Some(value) = line.strip_prefix("MemAvailable:") {
                available = value
                    .split_whitespace()
                    .next()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0);
            }
        }
    }
    (total, available)
}

fn disk_info_bytes(path: &str) -> (u64, u64) {
    let Ok(c_path) = CString::new(path) else {
        return (0, 0);
    };
    let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    let rc = unsafe { libc::statvfs(c_path.as_ptr(), stat.as_mut_ptr()) };
    if rc != 0 {
        return (0, 0);
    }
    let stat = unsafe { stat.assume_init() };
    let block_size = stat.f_frsize;
    (
        stat.f_blocks.saturating_mul(block_size),
        stat.f_bavail.saturating_mul(block_size),
    )
}

fn weather_response(request: &[u8]) -> Result<Vec<u8>> {
    let req = decode_weather_req(request)?;
    let body = json!({
        "source": "monsgeek-linux-bridge",
        "language": req.language,
        "address": req.address,
        "weather": [],
        "error": "live weather is not implemented in the Rust bridge yet"
    });
    Ok(protobuf_string(1, &body.to_string()))
}

fn ota_progress(error: &str, progress: f32) -> Vec<u8> {
    [protobuf_float(1, progress), protobuf_string(2, error)].concat()
}

async fn mac_like_send_feature(
    app: &Arc<AppState>,
    device_path: &str,
    payload: &[u8; REPORT_BYTES],
) -> Result<()> {
    if !app.config.mac_send_settle.is_zero() {
        tokio::time::sleep(app.config.mac_send_settle).await;
    }
    hid_send_feature(device_path, payload)?;
    if !app.config.mac_send_settle.is_zero() {
        tokio::time::sleep(app.config.mac_send_settle).await;
    }
    Ok(())
}

async fn mac_like_read_feature(app: &Arc<AppState>, device_path: &str) -> Result<Vec<u8>> {
    let attempts = app.config.mac_read_attempts.max(1);
    let mut last = Vec::new();
    for attempt in 0..attempts {
        if attempt == 0 {
            if !app.config.mac_send_settle.is_zero() {
                tokio::time::sleep(app.config.mac_send_settle).await;
            }
        } else if !app.config.mac_read_poll.is_zero() {
            tokio::time::sleep(app.config.mac_read_poll).await;
        }
        last = hid_get_feature(device_path)?;
        let devices = app.devices.lock().await;
        let transient = devices
            .get(device_path)
            .map(|state| is_transient_calibration_read(state, &last))
            .unwrap_or(false);
        drop(devices);
        if !transient {
            return Ok(last);
        }
    }
    Ok(last)
}

fn hidraw_ioctl(dir: u64, nr: u64, size: u64) -> libc::c_ulong {
    const IOC_NRBITS: u64 = 8;
    const IOC_TYPEBITS: u64 = 8;
    const IOC_SIZEBITS: u64 = 14;
    const IOC_NRSHIFT: u64 = 0;
    const IOC_TYPESHIFT: u64 = IOC_NRSHIFT + IOC_NRBITS;
    const IOC_SIZESHIFT: u64 = IOC_TYPESHIFT + IOC_TYPEBITS;
    const IOC_DIRSHIFT: u64 = IOC_SIZESHIFT + IOC_SIZEBITS;
    ((dir << IOC_DIRSHIFT)
        | (size << IOC_SIZESHIFT)
        | ((b'H' as u64) << IOC_TYPESHIFT)
        | (nr << IOC_NRSHIFT)) as libc::c_ulong
}

fn hid_send_feature(device_path: &str, payload: &[u8; REPORT_BYTES]) -> Result<()> {
    let file = open_hidraw(device_path, libc::O_RDWR | libc::O_NONBLOCK)?;
    let mut report = [0u8; REPORT_BYTES + 1];
    report[1..].copy_from_slice(payload);
    let request = hidraw_ioctl(3, 0x06, report.len() as u64);
    let rc = unsafe { libc::ioctl(file.as_raw_fd(), request, report.as_mut_ptr()) };
    if rc < 0 {
        return Err(anyhow!(
            "HIDIOCSFEATURE {device_path}: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

fn hid_get_feature(device_path: &str) -> Result<Vec<u8>> {
    let file = open_hidraw(device_path, libc::O_RDWR | libc::O_NONBLOCK)?;
    let mut report = [0u8; REPORT_BYTES + 1];
    let request = hidraw_ioctl(3, 0x07, report.len() as u64);
    let rc = unsafe { libc::ioctl(file.as_raw_fd(), request, report.as_mut_ptr()) };
    if rc < 0 {
        return Err(anyhow!(
            "HIDIOCGFEATURE {device_path}: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(report[1..].to_vec())
}

fn open_hidraw(device_path: &str, flags: i32) -> Result<OwnedFd> {
    let c_path = CString::new(device_path)?;
    let fd = unsafe { libc::open(c_path.as_ptr(), flags) };
    if fd < 0 {
        return Err(anyhow!(
            "open {device_path}: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

async fn update_write_state(app: &Arc<AppState>, device_path: &str, report: &[u8; REPORT_BYTES]) {
    let mut devices = app.devices.lock().await;
    let state = devices
        .entry(device_path.into())
        .or_insert_with(|| DeviceState::new(device_path.into()));
    state.last_command = Some(report[0]);
    match report[0] {
        FEA_CMD_SET_MAGNETISM_REPORT => {
            state.magnetism_report.active = report[1] != 0;
            if state.magnetism_report.active {
                let _ = app.vendor_events.send(vec![
                    0x00,
                    FEA_CMD_SET_MAGNETISM_REPORT,
                    0x00,
                    0x00,
                    0x00,
                ]);
            } else {
                let _ = app.vendor_events.send(vec![
                    0x00,
                    FEA_CMD_SET_MAGNETISM_REPORT,
                    0x00,
                    0x00,
                    0x00,
                ]);
            }
        }
        FEA_CMD_SET_MAGNETISM_CAL => {
            state.calibration.active = report[1] != 0;
            if state.calibration.active {
                reset_calibration_cache(state);
                state.calibration.input_ignore_until =
                    Some(Instant::now() + app.config.calibration_input_stabilize);
            } else {
                reset_calibration_state(state);
            }
        }
        FEA_CMD_SET_MAGNETISM_CALMAX => {
            state.calibration.maximum = report[1] != 0;
            if state.calibration.maximum {
                reset_calibration_cache(state);
                state.calibration.input_ignore_until =
                    Some(Instant::now() + app.config.calibration_input_stabilize);
            } else {
                reset_calibration_state(state);
            }
        }
        FEA_CMD_GET_MAGNETISM_BY_ARR => {
            state.calibration.travel_reads += 1;
            state.last_magnetism_read = Some(MagnetismRead {
                kind: report[1],
                page: report[3] as usize,
            });
            note_calibration_travel_read(&app.config, state, report[1], device_path);
        }
        _ => {}
    }
}

fn reset_calibration_state(state: &mut DeviceState) {
    state.calibration.active = false;
    state.calibration.maximum = false;
    state.calibration.travel_reads = 0;
    reset_calibration_cache(state);
    state.last_magnetism_read = None;
}

fn reset_calibration_cache(state: &mut DeviceState) {
    state.calibration.input_max.fill(0);
    state.calibration.input_active_until = None;
    state.calibration.input_ignore_until = None;
    state.calibration.pending_input = None;
    state.calibration.press = PressState::default();
}

fn note_calibration_travel_read(
    config: &Config,
    state: &mut DeviceState,
    kind: u8,
    device_path: &str,
) {
    if !config.calibration_input_cache
        || kind != MAGNETISM_TRAVEL_VALUES
        || !state.calibration.maximum
    {
        return;
    }
    let now = Instant::now();
    if state
        .calibration
        .input_active_until
        .is_some_and(|until| now > until)
    {
        state.calibration.input_max.fill(0);
    }
    state.calibration.input_active_until = Some(now + config.calibration_input_cache_ttl);
    state.calibration.device_path = device_path.into();
}

async fn start_vendor_input_readers(app: Arc<AppState>) {
    if !app.config.vendor_input_reader {
        return;
    }
    for path in app.config.vendor_hidraws.clone() {
        let mut readers = app.vendor_readers.lock().await;
        if readers.contains_key(&path) {
            continue;
        }
        let app_clone = app.clone();
        let path_clone = path.clone();
        let handle = tokio::spawn(async move {
            if let Err(error) = vendor_input_loop(app_clone, path_clone.clone()).await {
                eprintln!("vendor input reader disabled for {path_clone}: {error}");
            }
        });
        readers.insert(path, handle);
    }
}

async fn vendor_input_loop(app: Arc<AppState>, device_path: String) -> Result<()> {
    let fd = open_hidraw(&device_path, libc::O_RDONLY | libc::O_NONBLOCK)?;
    let async_fd = tokio::io::unix::AsyncFd::new(fd)?;
    let mut buffer = [0u8; REPORT_BYTES];
    loop {
        let mut guard = async_fd.readable().await?;
        let mut saw_would_block = false;
        loop {
            match read_fd(async_fd.get_ref().as_raw_fd(), &mut buffer) {
                Ok(0) => break,
                Ok(n) => {
                    let report = buffer[..n].to_vec();
                    if app.config.trace_hid || app.config.trace_focus {
                        println!("vendor input {device_path} {}", hex(&report));
                    }
                    note_physical_keyboard_input(&app, &report).await;
                    note_vendor_input_report(&app, &report).await;
                    if should_forward_vendor_input_report(&app, &report).await {
                        let _ = app.vendor_events.send(report);
                    }
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => {
                    saw_would_block = true;
                    break;
                }
                Err(error) => return Err(error).with_context(|| format!("read {device_path}")),
            }
        }
        if saw_would_block {
            guard.clear_ready();
        }
    }
}

fn read_fd(fd: RawFd, buffer: &mut [u8]) -> std::io::Result<usize> {
    let rc = unsafe { libc::read(fd, buffer.as_mut_ptr().cast(), buffer.len()) };
    if rc < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(rc as usize)
    }
}

async fn should_forward_vendor_input_report(app: &Arc<AppState>, report: &[u8]) -> bool {
    if magnet_travel_report_offset(report).is_none() {
        return true;
    }
    let devices = app.devices.lock().await;
    devices
        .values()
        .any(|state| state.magnetism_report.active && !state.calibration.maximum)
}

async fn note_physical_keyboard_input(app: &Arc<AppState>, report: &[u8]) {
    if !is_boot_keyboard_input_report(report) {
        return;
    }
    let has_pressed_key = report.iter().any(|byte| *byte != 0);
    let now = Instant::now();
    let mut last = app.last_boot_keyboard_input.lock().await;
    let changed = report != last.as_slice();
    *last = report.to_vec();
    drop(last);

    *app.physical_keyboard_input_active_until.lock().await = if has_pressed_key {
        Some(now + app.config.calibration_physical_input_grace)
    } else {
        None
    };

    let mut devices = app.devices.lock().await;
    if !has_pressed_key {
        for state in devices.values_mut() {
            state.calibration.press = PressState::default();
        }
        return;
    }
    for state in devices.values_mut() {
        if state.calibration.maximum
            && state
                .calibration
                .input_active_until
                .is_some_and(|until| now <= until)
            && changed
        {
            state.calibration.press = PressState {
                active: true,
                key_index: None,
                started_at: now,
                candidates: HashMap::new(),
            };
            state.calibration.pending_input = None;
        }
    }
}

async fn note_vendor_input_report(app: &Arc<AppState>, report: &[u8]) {
    if !app.config.calibration_input_cache {
        return;
    }
    let Some(offset) = magnet_travel_report_offset(report) else {
        return;
    };
    if offset + 4 > report.len() {
        return;
    }
    let travel = u16::from_le_bytes([report[offset + 1], report[offset + 2]]);
    if travel == 0 {
        return;
    }
    let key_index = report[offset + 3] as usize;
    if key_index >= CALIBRATION_MAX_KEYS {
        return;
    }
    let now = Instant::now();
    if app
        .physical_keyboard_input_active_until
        .lock()
        .await
        .is_none_or(|until| now > until)
    {
        return;
    }
    let mut devices = app.devices.lock().await;
    for state in devices.values_mut() {
        if !state.calibration.maximum
            || state
                .calibration
                .input_active_until
                .is_none_or(|until| now > until)
        {
            continue;
        }
        if state
            .calibration
            .input_ignore_until
            .is_some_and(|until| now < until)
        {
            continue;
        }
        if !select_calibration_press_key(&app.config, state, key_index, travel, now) {
            continue;
        }
        let pending = state.calibration.pending_input;
        if pending.is_none_or(|p| {
            p.key_index != key_index
                || now.duration_since(p.time) > app.config.calibration_input_confirm
        }) {
            state.calibration.pending_input = Some(PendingInput {
                key_index,
                travel,
                time: now,
                count: 1,
            });
            continue;
        }
        let pending = pending.unwrap();
        let next = PendingInput {
            key_index,
            travel: pending.travel.max(travel),
            time: now,
            count: pending.count + 1,
        };
        state.calibration.pending_input = Some(next);
        if next.count >= 2 && next.travel > state.calibration.input_max[key_index] {
            state.calibration.input_max[key_index] = next.travel;
        }
    }
}

fn select_calibration_press_key(
    config: &Config,
    state: &mut DeviceState,
    key_index: usize,
    travel: u16,
    now: Instant,
) -> bool {
    let press = &mut state.calibration.press;
    if !press.active {
        return false;
    }
    if let Some(selected) = press.key_index {
        return selected == key_index;
    }
    let candidate = press.candidates.entry(key_index).or_default();
    candidate.count += 1;
    candidate.max = candidate.max.max(travel);
    candidate.last_at = Some(now);
    if now.duration_since(press.started_at) < config.calibration_press_select {
        return false;
    }
    let mut selected_key = None;
    let mut selected = Candidate::default();
    for (candidate_key, value) in &press.candidates {
        if value.count < 2 {
            continue;
        }
        if value.count > selected.count
            || (value.count == selected.count && value.max > selected.max)
            || (value.count == selected.count
                && value.max == selected.max
                && value.last_at > selected.last_at)
        {
            selected_key = Some(*candidate_key);
            selected = *value;
        }
    }
    press.key_index = selected_key;
    selected_key == Some(key_index)
}

fn is_boot_keyboard_input_report(report: &[u8]) -> bool {
    report.len() == 8 && magnet_travel_report_offset(report).is_none()
}

fn magnet_travel_report_offset(report: &[u8]) -> Option<usize> {
    if report.len() >= 5 && report[1] == FEA_CMD_SET_MAGNETISM_REPORT {
        Some(1)
    } else if report.len() >= 4 && report[0] == FEA_CMD_SET_MAGNETISM_REPORT {
        Some(0)
    } else {
        None
    }
}

fn cached_calibration_input_payload(
    app: &Arc<AppState>,
    state: &DeviceState,
    payload: &[u8],
) -> Vec<u8> {
    let clean_payload = if is_calibration_control_echo(payload) {
        vec![0; payload.len()]
    } else {
        payload.to_vec()
    };
    let Some(query) = state.last_magnetism_read else {
        return clean_payload;
    };
    if !app.config.calibration_input_cache
        || !state.calibration.maximum
        || query.kind != MAGNETISM_TRAVEL_VALUES
        || clean_payload.len() < 2
        || state
            .calibration
            .input_active_until
            .is_none_or(|until| Instant::now() > until)
    {
        return clean_payload;
    }
    let page = query.page.min(3);
    let base_key = page * CALIBRATION_KEYS_PER_PAGE;
    let mut out = clean_payload.clone();
    let slots = CALIBRATION_KEYS_PER_PAGE.min(clean_payload.len() / 2);
    for slot in 0..slots {
        let offset = slot * 2;
        let cached = state.calibration.input_max[base_key + slot];
        if cached == 0 {
            continue;
        }
        let raw = u16::from_le_bytes([out[offset], out[offset + 1]]);
        if cached > raw {
            out[offset..offset + 2].copy_from_slice(&cached.to_le_bytes());
        }
    }
    out
}

fn monotonic_calibration_payload(
    _app: &Arc<AppState>,
    state: &mut DeviceState,
    payload: &[u8],
) -> Vec<u8> {
    if !env_flag("MONSGEEK_CALIBRATION_HOLD") || !state.calibration.maximum {
        return payload.to_vec();
    }
    payload.to_vec()
}

fn is_calibration_control_echo(payload: &[u8]) -> bool {
    payload.len() >= 2
        && payload[0] == FEA_CMD_SET_MAGNETISM_REPORT
        && (payload[1] == 0 || payload[1] == 1)
}

fn has_cached_calibration_page(state: &DeviceState) -> bool {
    let page = state
        .last_magnetism_read
        .map(|q| q.page.min(3))
        .unwrap_or(0);
    let base_key = page * CALIBRATION_KEYS_PER_PAGE;
    state.calibration.input_max[base_key..base_key + CALIBRATION_KEYS_PER_PAGE]
        .iter()
        .any(|value| *value != 0)
}

fn is_transient_calibration_read(state: &DeviceState, payload: &[u8]) -> bool {
    state.last_magnetism_read.is_some_and(|query| {
        query.kind == MAGNETISM_TRAVEL_VALUES
            && (is_calibration_control_echo(payload)
                || (payload.iter().all(|byte| *byte == 0) && has_cached_calibration_page(state)))
    })
}

fn res_send(err: &str) -> Vec<u8> {
    if err.is_empty() {
        Vec::new()
    } else {
        protobuf_string(1, err)
    }
}

fn res_read(payload: &[u8], err: &str) -> Vec<u8> {
    let mut fields = Vec::new();
    if !err.is_empty() {
        fields.extend(protobuf_string(1, err));
    }
    if !payload.is_empty() {
        fields.extend(protobuf_bytes(2, payload));
    }
    fields
}

fn vender_message(payload: &[u8]) -> Vec<u8> {
    if payload.is_empty() {
        Vec::new()
    } else {
        protobuf_bytes(1, payload)
    }
}

fn vender_light_payload(light: &SetLight) -> Vec<u8> {
    vec![0x00, 0x0f, u8::from(light.light_type != LIGHT_OTHER), 0x00]
}

fn load_db(path: &PathBuf) -> Result<Value> {
    if !path.exists() {
        return Ok(json!({ "dbs": {} }));
    }
    let text = fs::read_to_string(path).with_context(|| format!("read DB {}", path.display()))?;
    let mut value: Value =
        serde_json::from_str(&text).with_context(|| format!("parse DB {}", path.display()))?;
    if !value.get("dbs").is_some_and(Value::is_object) {
        value["dbs"] = Value::Object(Map::new());
    }
    Ok(value)
}

fn save_db(path: &PathBuf, value: &Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create DB directory {}", parent.display()))?;
    }
    let tmp = path.with_extension("json.tmp");
    let text = serde_json::to_string_pretty(value)?;
    fs::write(&tmp, text).with_context(|| format!("write DB temp {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("replace DB {}", path.display()))?;
    Ok(())
}

fn db_bucket_mut<'a>(db: &'a mut Value, dbpath: &str) -> &'a mut Map<String, Value> {
    if !db.get("dbs").is_some_and(Value::is_object) {
        db["dbs"] = Value::Object(Map::new());
    }
    let dbs = db["dbs"].as_object_mut().expect("dbs object");
    dbs.entry(dbpath.to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if !dbs.get(dbpath).is_some_and(Value::is_object) {
        dbs.insert(dbpath.to_string(), Value::Object(Map::new()));
    }
    dbs.get_mut(dbpath)
        .and_then(Value::as_object_mut)
        .expect("db bucket object")
}

fn db_bucket<'a>(db: &'a Value, dbpath: &str) -> Option<&'a Map<String, Value>> {
    db.get("dbs")?.get(dbpath)?.as_object()
}

fn store_db_item(path: &PathBuf, req: &DbMessage) -> Result<()> {
    let mut db = load_db(path)?;
    let bucket = db_bucket_mut(&mut db, &req.dbpath);
    bucket.insert(
        STANDARD.encode(&req.key),
        Value::String(STANDARD.encode(&req.value)),
    );
    save_db(path, &db)
}

fn stored_db_item(path: &PathBuf, req: &DbMessage) -> Result<Vec<u8>> {
    let db = load_db(path)?;
    let key = STANDARD.encode(&req.key);
    let Some(value) = db_bucket(&db, &req.dbpath)
        .and_then(|bucket| bucket.get(&key))
        .and_then(Value::as_str)
    else {
        return Ok(protobuf_string(2, "linux bridge has no stored item"));
    };
    let value = STANDARD.decode(value).unwrap_or_default();
    Ok(protobuf_bytes(1, &value))
}

fn stored_db_list(path: &PathBuf, dbpath: &str, keys: bool) -> Result<Vec<u8>> {
    let db = load_db(path)?;
    let mut out = Vec::new();
    if let Some(bucket) = db_bucket(&db, dbpath) {
        for (key, value) in bucket {
            let encoded = if keys {
                key.as_str()
            } else {
                value.as_str().unwrap_or_default()
            };
            let decoded = STANDARD.decode(encoded).unwrap_or_default();
            out.extend(protobuf_bytes(1, &decoded));
        }
    }
    Ok(out)
}

fn delete_db_item(path: &PathBuf, req: &DbMessage) -> Result<()> {
    let mut db = load_db(path)?;
    let bucket = db_bucket_mut(&mut db, &req.dbpath);
    bucket.remove(&STANDARD.encode(&req.key));
    save_db(path, &db)
}
