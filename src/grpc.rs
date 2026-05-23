use anyhow::{anyhow, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use percent_encoding::{percent_encode, NON_ALPHANUMERIC};

pub fn encode_varint(mut value: u64) -> Vec<u8> {
    let mut out = Vec::new();
    while value > 0x7f {
        out.push(((value & 0x7f) as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
    out
}

pub fn protobuf_string(field: u32, value: &str) -> Vec<u8> {
    protobuf_bytes(field, value.as_bytes())
}

pub fn protobuf_varint(field: u32, value: u64) -> Vec<u8> {
    [encode_varint((field as u64) << 3), encode_varint(value)].concat()
}

pub fn protobuf_bytes(field: u32, value: &[u8]) -> Vec<u8> {
    [
        encode_varint(((field as u64) << 3) | 2),
        encode_varint(value.len() as u64),
        value.to_vec(),
    ]
    .concat()
}

pub fn protobuf_float(field: u32, value: f32) -> Vec<u8> {
    [
        encode_varint(((field as u64) << 3) | 5),
        value.to_le_bytes().to_vec(),
    ]
    .concat()
}

pub fn grpc_frame(flag: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = vec![flag, 0, 0, 0, 0];
    out[1..5].copy_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(payload);
    out
}

pub fn grpc_text_response(message: &[u8], status: u32, status_message: &str) -> String {
    let trailer = format!(
        "grpc-status:{status}\r\ngrpc-message:{}\r\n",
        percent_encode(status_message.as_bytes(), NON_ALPHANUMERIC)
    );
    STANDARD.encode([grpc_frame(0, message), grpc_frame(0x80, trailer.as_bytes())].concat())
}

pub fn decode_grpc_text_body(body: &[u8]) -> Vec<u8> {
    let Ok(raw) = STANDARD.decode(body) else {
        return Vec::new();
    };
    if raw.len() < 5 || raw[0] != 0 {
        return Vec::new();
    }
    let len = u32::from_be_bytes([raw[1], raw[2], raw[3], raw[4]]) as usize;
    raw.get(5..5 + len).unwrap_or_default().to_vec()
}

pub fn decode_varint(bytes: &[u8], index: &mut usize) -> Result<u64> {
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

pub fn decode_len(bytes: &[u8], index: &mut usize) -> Result<Vec<u8>> {
    let len = decode_varint(bytes, index)? as usize;
    let end = *index + len;
    if end > bytes.len() {
        return Err(anyhow!("truncated protobuf bytes"));
    }
    let out = bytes[*index..end].to_vec();
    *index = end;
    Ok(out)
}

pub fn skip_field(bytes: &[u8], index: &mut usize, wire_type: u64) -> Result<()> {
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
