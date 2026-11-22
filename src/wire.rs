//! Минимальная реализация сетевого формата Mindustry 160.8 (Arc net).
//! Без внешних зависимостей: только std.

use std::io::{self, Read};

// ---------------------------------------------------------------------------
// Примитивы big-endian (как Java DataOutputStream)
// ---------------------------------------------------------------------------

pub fn put_u16(buf: &mut Vec<u8>, v: u16) {
    buf.extend_from_slice(&v.to_be_bytes());
}
pub fn put_i16(buf: &mut Vec<u8>, v: i16) {
    buf.extend_from_slice(&v.to_be_bytes());
}
pub fn put_i32(buf: &mut Vec<u8>, v: i32) {
    buf.extend_from_slice(&v.to_be_bytes());
}
pub fn put_i64(buf: &mut Vec<u8>, v: i64) {
    buf.extend_from_slice(&v.to_be_bytes());
}
pub fn put_f32(buf: &mut Vec<u8>, v: f32) {
    put_i32(buf, v.to_bits() as i32);
}
pub fn put_bool(buf: &mut Vec<u8>, v: bool) {
    buf.push(if v { 1 } else { 0 });
}

pub fn get_u16(data: &[u8], pos: &mut usize) -> Option<u16> {
    if *pos + 2 > data.len() {
        return None;
    }
    let v = u16::from_be_bytes([data[*pos], data[*pos + 1]]);
    *pos += 2;
    Some(v)
}

pub fn get_i32(data: &[u8], pos: &mut usize) -> Option<i32> {
    if *pos + 4 > data.len() {
        return None;
    }
    let v = i32::from_be_bytes([
        data[*pos],
        data[*pos + 1],
        data[*pos + 2],
        data[*pos + 3],
    ]);
    *pos += 4;
    Some(v)
}

pub fn get_u8(data: &[u8], pos: &mut usize) -> Option<u8> {
    let v = *data.get(*pos)?;
    *pos += 1;
    Some(v)
}

pub fn get_i16(data: &[u8], pos: &mut usize) -> Option<i16> {
    if *pos + 2 > data.len() {
        return None;
    }
    let v = i16::from_be_bytes([data[*pos], data[*pos + 1]]);
    *pos += 2;
    Some(v)
}

pub fn get_i64(data: &[u8], pos: &mut usize) -> Option<i64> {
    if *pos + 8 > data.len() {
        return None;
    }
    let mut b = [0u8; 8];
    b.copy_from_slice(&data[*pos..*pos + 8]);
    *pos += 8;
    Some(i64::from_be_bytes(b))
}

pub fn get_f32(data: &[u8], pos: &mut usize) -> Option<f32> {
    Some(f32::from_bits(get_i32(data, pos)? as u32))
}

pub fn get_f64(data: &[u8], pos: &mut usize) -> Option<f64> {
    Some(f64::from_bits(get_i64(data, pos)? as u64))
}

pub fn get_bool(data: &[u8], pos: &mut usize) -> Option<bool> {
    Some(get_u8(data, pos)? != 0)
}

/// Пропустить Java writeUTF (u16 длины + байты).
pub fn skip_utf(data: &[u8], pos: &mut usize) -> bool {
    let len = match get_u16(data, pos) {
        Some(v) => v as usize,
        None => return false,
    };
    if *pos + len > data.len() {
        return false;
    }
    *pos += len;
    true
}

/// Прочитать Java writeUTF как строку.
pub fn get_utf_string(data: &[u8], pos: &mut usize) -> Option<String> {
    let len = get_u16(data, pos)? as usize;
    if *pos + len > data.len() {
        return None;
    }
    let s = decode_modified_utf8(&data[*pos..*pos + len]);
    *pos += len;
    Some(s)
}

// ---------------------------------------------------------------------------
// Java "modified UTF-8" (DataOutput.writeUTF) и TypeIO.writeString
// ---------------------------------------------------------------------------

fn push_code(buf: &mut Vec<u8>, cp: u32) {
    if cp <= 0x7f {
        buf.push(cp as u8);
    } else if cp <= 0x7ff {
        buf.push(0xc0 | (cp >> 6) as u8);
        buf.push(0x80 | (cp & 0x3f) as u8);
    } else {
        buf.push(0xe0 | (cp >> 12) as u8);
        buf.push(0x80 | ((cp >> 6) & 0x3f) as u8);
        buf.push(0x80 | (cp & 0x3f) as u8);
    }
}

/// Кодирует строку как Java DataOutput.writeUTF: u16 длины + modified UTF-8.
pub fn put_utf(buf: &mut Vec<u8>, s: &str) {
    let mut bytes = Vec::new();
    for ch in s.chars() {
        let cp = ch as u32;
        if cp == 0 {
            // В модифицированном UTF-8 NUL кодируется как C0 80.
            bytes.extend_from_slice(&[0xc0, 0x80]);
        } else if cp <= 0xffff {
            push_code(&mut bytes, cp);
        } else {
            // Дополнительные символы → пара суррогатов.
            let v = cp - 0x10000;
            push_code(&mut bytes, 0xd800 + (v >> 10));
            push_code(&mut bytes, 0xdc00 + (v & 0x3ff));
        }
    }
    put_u16(buf, bytes.len() as u16);
    buf.extend_from_slice(&bytes);
}

/// TypeIO.writeString: u8 = 1 (не null) + writeUTF.
pub fn put_typeio_string(buf: &mut Vec<u8>, s: &str) {
    buf.push(1);
    put_utf(buf, s);
}

/// Декодирует Java modified UTF-8 (упрощённо, для строк кика).
fn decode_modified_utf8(bytes: &[u8]) -> String {
    let mut out = String::new();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b < 0x80 {
            out.push(b as char);
            i += 1;
        } else if b & 0xe0 == 0xc0 && i + 1 < bytes.len() {
            let cp = (((b & 0x1f) as u32) << 6) | ((bytes[i + 1] & 0x3f) as u32);
            out.push(char::from_u32(cp).unwrap_or('\u{fffd}'));
            i += 2;
        } else if b & 0xf0 == 0xe0 && i + 2 < bytes.len() {
            let cp = (((b & 0x0f) as u32) << 12)
                | (((bytes[i + 1] & 0x3f) as u32) << 6)
                | ((bytes[i + 2] & 0x3f) as u32);
            // Объединяем суррогатную пару, если она есть.
            if (0xd800..=0xdbff).contains(&cp)
                && i + 5 < bytes.len()
                && bytes[i + 3] & 0xf0 == 0xe0
            {
                let lo = (((bytes[i + 3] & 0x0f) as u32) << 12)
                    | (((bytes[i + 4] & 0x3f) as u32) << 6)
                    | ((bytes[i + 5] & 0x3f) as u32);
                if (0xdc00..=0xdfff).contains(&lo) {
                    let full = 0x10000 + ((cp - 0xd800) << 10) + (lo - 0xdc00);
                    out.push(char::from_u32(full).unwrap_or('\u{fffd}'));
                    i += 6;
                    continue;
                }
            }
            out.push(char::from_u32(cp).unwrap_or('\u{fffd}'));
            i += 3;
        } else {
            out.push('\u{fffd}');
            i += 1;
        }
    }
    out
}

/// Читает TypeIO-строку из буфера; None если \u{0}u8 == 0 (null).
pub fn get_typeio_string(data: &[u8], pos: &mut usize) -> Option<String> {
    if *pos >= data.len() {
        return None;
    }
    let present = data[*pos];
    *pos += 1;
    if present == 0 {
        return None;
    }
    let len = get_u16(data, pos)? as usize;
    if *pos + len > data.len() {
        return None;
    }
    let s = decode_modified_utf8(&data[*pos..*pos + len]);
    *pos += len;
    Some(s)
}

// ---------------------------------------------------------------------------
// Base64 и CRC32 (для UUID/USID)
// ---------------------------------------------------------------------------

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn b64_encode(data: &[u8]) -> String {
    let mut out = String::new();
    let mut i = 0;
    while i < data.len() {
        let a = data[i] as u32;
        let b = if i + 1 < data.len() { data[i + 1] as u32 } else { 0 };
        let c = if i + 2 < data.len() { data[i + 2] as u32 } else { 0 };
        let n = (a << 16) | (b << 8) | c;
        out.push(B64[((n >> 18) & 63) as usize] as char);
        out.push(B64[((n >> 12) & 63) as usize] as char);
        out.push(if i + 1 < data.len() {
            B64[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if i + 2 < data.len() {
            B64[(n & 63) as usize] as char
        } else {
            '='
        });
        i += 3;
    }
    out
}

pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

// ---------------------------------------------------------------------------
// LZ4 block decompression (для сжатых серверных пакетов)
// ---------------------------------------------------------------------------

pub fn lz4_decompress(src: &[u8]) -> Option<Vec<u8>> {
    let mut dst: Vec<u8> = Vec::new();
    let mut i = 0usize;
    while i < src.len() {
        let token = src[i];
        i += 1;
        let mut lit = (token >> 4) as usize;
        if lit == 15 {
            loop {
                if i >= src.len() {
                    return None;
                }
                let b = src[i] as usize;
                i += 1;
                lit += b;
                if b != 255 {
                    break;
                }
            }
        }
        if i + lit > src.len() {
            return None;
        }
        dst.extend_from_slice(&src[i..i + lit]);
        i += lit;
        if i == src.len() {
            break;
        }
        if i + 2 > src.len() {
            return None;
        }
        let offset = u16::from_le_bytes([src[i], src[i + 1]]) as usize;
        i += 2;
        if offset == 0 || offset > dst.len() {
            return None;
        }
        let mut mlen = (token & 0x0f) as usize + 4;
        if token & 0x0f == 15 {
            loop {
                if i >= src.len() {
                    return None;
                }
                let b = src[i] as usize;
                i += 1;
                mlen += b;
                if b != 255 {
                    break;
                }
            }
        }
        for _ in 0..mlen {
            let byte = dst[dst.len() - offset];
            dst.push(byte);
        }
    }
    Some(dst)
}

// ---------------------------------------------------------------------------
// Кадры TCP / тела пакетов
// ---------------------------------------------------------------------------

/// Читает один TCP-кадр: u16 длины (big-endian) + payload.
pub fn read_tcp_frame<R: Read>(r: &mut R) -> io::Result<Vec<u8>> {
    let mut len_bytes = [0u8; 2];
    r.read_exact(&mut len_bytes)?;
    let len = u16::from_be_bytes(len_bytes) as usize;
    let mut payload = vec![0u8; len];
    r.read_exact(&mut payload)?;
    Ok(payload)
}

/// Тело обычного пакета: id + u16 длины + u8 сжатия(0) + данные.
pub fn packet_body(id: u8, payload: &[u8]) -> Vec<u8> {
    let mut body = Vec::with_capacity(payload.len() + 4);
    body.push(id);
    put_u16(&mut body, payload.len() as u16);
    body.push(0); // без сжатия
    body.extend_from_slice(payload);
    body
}

/// Тело служебного (framework) пакета: 0xFE + subtype + данные.
pub fn framework_body(subtype: u8, extra: &[u8]) -> Vec<u8> {
    let mut body = Vec::with_capacity(2 + extra.len());
    body.push(0xfe);
    body.push(subtype);
    body.extend_from_slice(extra);
    body
}

/// Готовый TCP-кадр с обычным пакетом.
pub fn tcp_frame(id: u8, payload: &[u8]) -> Vec<u8> {
    let body = packet_body(id, payload);
    let mut frame = Vec::with_capacity(body.len() + 2);
    put_u16(&mut frame, body.len() as u16);
    frame.extend_from_slice(&body);
    frame
}

/// Готовый TCP-кадр со служебным пакетом.
pub fn tcp_framework_frame(subtype: u8, extra: &[u8]) -> Vec<u8> {
    let body = framework_body(subtype, extra);
    let mut frame = Vec::with_capacity(body.len() + 2);
    put_u16(&mut frame, body.len() as u16);
    frame.extend_from_slice(&body);
    frame
}

/// Разобранное входящее тело пакета.
pub struct Inbound {
    pub id: u8,
    pub framework: Option<u8>,
    pub data: Vec<u8>,
    pub extra: Vec<u8>,
}

pub fn parse_body(body: &[u8]) -> Option<Inbound> {
    if body.is_empty() {
        return None;
    }
    if body[0] == 0xfe {
        if body.len() < 2 {
            return None;
        }
        return Some(Inbound {
            id: 0xfe,
            framework: Some(body[1]),
            data: Vec::new(),
            extra: body[2..].to_vec(),
        });
    }
    let id = body[0];
    let mut pos = 1;
    let len = get_u16(body, &mut pos)? as usize;
    if pos >= body.len() {
        return None;
    }
    let compression = body[pos];
    pos += 1;
    if pos + len > body.len() {
        return None;
    }
    let raw = &body[pos..pos + len];
    let data = if compression == 1 {
        lz4_decompress(raw)?
    } else {
        raw.to_vec()
    };
    Some(Inbound {
        id,
        framework: None,
        data,
        extra: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf_latin() {
        let mut b = Vec::new();
        put_utf(&mut b, "Hi");
        assert_eq!(b, vec![0, 2, b'H', b'i']);
    }

    #[test]
    fn typeio_string() {
        let mut b = Vec::new();
        put_typeio_string(&mut b, "en");
        assert_eq!(b, vec![1, 0, 2, b'e', b'n']);
        let mut p = 0;
        assert_eq!(get_typeio_string(&b, &mut p).unwrap(), "en");
    }

    #[test]
    fn base64_8bytes() {
        assert_eq!(b64_encode(&[0, 1, 2, 3, 4, 5, 6, 7]).len(), 12);
    }

    #[test]
    fn frame_roundtrip() {
        let f = tcp_framework_frame(4, &[1, 2, 3, 4]);
        assert_eq!(f.len(), 2 + 2 + 4);
        let mut p = 0;
        let len = get_u16(&f, &mut p).unwrap() as usize;
        assert_eq!(len, f.len() - 2);
        let inb = parse_body(&f[2..]).unwrap();
        assert_eq!(inb.framework, Some(4));
        assert_eq!(inb.extra, vec![1, 2, 3, 4]);
    }
}
