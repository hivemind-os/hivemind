//! Minimal protobuf wire-format decoder.
//!
//! We only need to extract specific fields from known message types, so a
//! full protobuf library is unnecessary. This module decodes the wire format
//! just enough to pull out field values by number.

/// A single protobuf field value decoded from the wire format.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub enum Value {
    Varint(u64),
    Fixed64(u64),
    Fixed32(u32),
    /// Length-delimited bytes (could be a string, embedded message, or packed repeated field).
    Bytes(Vec<u8>),
}

/// A decoded field: (field_number, value).
pub type Field = (u32, Value);

/// Decode a varint from the buffer, returning (value, bytes_consumed).
pub fn decode_varint(buf: &[u8]) -> Option<(u64, usize)> {
    let mut value: u64 = 0;
    let mut shift = 0u32;
    for (i, &byte) in buf.iter().enumerate() {
        if shift >= 70 {
            return None; // overflow protection
        }
        value |= ((byte & 0x7F) as u64) << shift;
        shift += 7;
        if byte & 0x80 == 0 {
            return Some((value, i + 1));
        }
    }
    None // incomplete varint
}

/// Decode all fields from a protobuf-encoded message buffer.
pub fn decode_fields(buf: &[u8]) -> Vec<Field> {
    let mut fields = Vec::new();
    let mut pos = 0;

    while pos < buf.len() {
        let (tag, n) = match decode_varint(&buf[pos..]) {
            Some(v) => v,
            None => break,
        };
        pos += n;

        let field_number = (tag >> 3) as u32;
        let wire_type = (tag & 0x07) as u8;

        match wire_type {
            0 => {
                // Varint
                let (val, n) = match decode_varint(&buf[pos..]) {
                    Some(v) => v,
                    None => break,
                };
                pos += n;
                fields.push((field_number, Value::Varint(val)));
            }
            1 => {
                // 64-bit (fixed64, sfixed64, double)
                if pos + 8 > buf.len() {
                    break;
                }
                let val = u64::from_le_bytes(buf[pos..pos + 8].try_into().unwrap());
                pos += 8;
                fields.push((field_number, Value::Fixed64(val)));
            }
            2 => {
                // Length-delimited (string, bytes, embedded message, packed repeated)
                let (len, n) = match decode_varint(&buf[pos..]) {
                    Some(v) => v,
                    None => break,
                };
                pos += n;
                let len = len as usize;
                if pos + len > buf.len() {
                    break;
                }
                let data = buf[pos..pos + len].to_vec();
                pos += len;
                fields.push((field_number, Value::Bytes(data)));
            }
            5 => {
                // 32-bit (fixed32, sfixed32, float)
                if pos + 4 > buf.len() {
                    break;
                }
                let val = u32::from_le_bytes(buf[pos..pos + 4].try_into().unwrap());
                pos += 4;
                fields.push((field_number, Value::Fixed32(val)));
            }
            3 | 4 => {
                // Start/end group (deprecated) — skip
                break;
            }
            _ => break,
        }
    }

    fields
}

/// Extract all `Bytes` values for a given field number (useful for repeated
/// length-delimited fields like `repeated string`).
pub fn get_repeated_bytes(fields: &[Field], field_number: u32) -> Vec<&[u8]> {
    fields
        .iter()
        .filter_map(|(num, val)| {
            if *num == field_number {
                if let Value::Bytes(ref data) = val {
                    return Some(data.as_slice());
                }
            }
            None
        })
        .collect()
}

/// Extract the first varint value for a given field number.
pub fn get_varint(fields: &[Field], field_number: u32) -> Option<u64> {
    fields.iter().find_map(|(num, val)| {
        if *num == field_number {
            if let Value::Varint(v) = val {
                return Some(*v);
            }
        }
        None
    })
}

/// Extract the first `Bytes` value for a given field number.
#[allow(dead_code)]
pub fn get_bytes(fields: &[Field], field_number: u32) -> Option<&[u8]> {
    fields.iter().find_map(|(num, val)| {
        if *num == field_number {
            if let Value::Bytes(ref data) = val {
                return Some(data.as_slice());
            }
        }
        None
    })
}

/// Decode a packed repeated uint64 field.
#[allow(dead_code)]
pub fn decode_packed_uint64(data: &[u8]) -> Vec<u64> {
    let mut result = Vec::new();
    let mut pos = 0;
    while pos < data.len() {
        if let Some((val, n)) = decode_varint(&data[pos..]) {
            result.push(val);
            pos += n;
        } else {
            break;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_varint_basic() {
        // 150 = 10010110 00000001
        assert_eq!(decode_varint(&[0x96, 0x01]), Some((150, 2)));
        // 1
        assert_eq!(decode_varint(&[0x01]), Some((1, 1)));
        // 300
        assert_eq!(decode_varint(&[0xAC, 0x02]), Some((300, 2)));
    }

    #[test]
    fn decode_fields_simple_message() {
        // field 1 = varint 150: tag = (1 << 3) | 0 = 0x08, value = 0x96 0x01
        // field 3 = string "testing": tag = (3 << 3) | 2 = 0x1a, len = 7
        let buf = [
            0x08, 0x96, 0x01, // field 1 = 150
            0x1a, 0x07, b't', b'e', b's', b't', b'i', b'n', b'g', // field 3 = "testing"
        ];
        let fields = decode_fields(&buf);
        assert_eq!(fields.len(), 2);
        assert_eq!(get_varint(&fields, 1), Some(150));
        let text = get_bytes(&fields, 3).unwrap();
        assert_eq!(std::str::from_utf8(text).unwrap(), "testing");
    }
}
