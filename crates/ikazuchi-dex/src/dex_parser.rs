//! # Pure-Rust Dalvik `classes.dex` Binary Parser
//!
//! Parses standard Android Dalvik Executable (`dex\n035\0` .. `dex\n040\0`) files without
//! any external dependencies or JVM:
//! - Validates the 112-byte DEX header.
//! - Decodes the `string_ids` table (ULEB128 length + MUTF-8 payload).
//! - Decodes the `type_ids` table (`descriptor_idx -> string`).
//! - Decodes the `class_defs` table (`class_idx`, `access_flags`, `superclass_idx`, `source_file_idx`).
//! - Identifies Mihon `Source` / `HttpSource` / `Multisrc` subclasses and extracts string literals.

use ikazuchi_core::{IkazuchiError, Result};
use serde::{Deserialize, Serialize};

/// Parsed representation of a `classes.dex` binary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DexFile {
    pub version: String,
    pub strings: Vec<String>,
    pub types: Vec<String>,
    pub classes: Vec<DexClassDef>,
}

/// A single class definition from the `class_defs` section of a DEX file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DexClassDef {
    /// JVM-style type descriptor, e.g. `"Leu/kanade/tachiyomi/extension/en/asurascans/AsuraScans;"`
    pub class_descriptor: String,
    /// Superclass descriptor, e.g. `"Leu/kanade/tachiyomi/multisrc/mangathemesia/MangaThemesia;"`
    pub superclass_descriptor: Option<String>,
    /// Access flags (e.g. `0x0001` = PUBLIC, `0x0010` = FINAL, `0x0400` = ABSTRACT)
    pub access_flags: u32,
    /// Optional source file name (e.g. `"AsuraScans.kt"`)
    pub source_file: Option<String>,
}

const NO_INDEX: u32 = 0xFFFF_FFFF;

impl DexFile {
    /// Parses a raw `classes.dex` byte slice.
    pub fn parse(data: &[u8]) -> Result<Self> {
        if data.len() < 112 {
            return Err(IkazuchiError::Parse(format!(
                "DEX buffer too small ({} bytes, minimum 112)",
                data.len()
            )));
        }

        if &data[0..4] != b"dex\n" || data[7] != 0 {
            return Err(IkazuchiError::Parse(
                "Invalid DEX magic header (expected 'dex\\n03x\\0')".to_string(),
            ));
        }

        let version = String::from_utf8_lossy(&data[4..7]).to_string();

        let string_ids_size = read_u32_le(data, 56)? as usize;
        let string_ids_off = read_u32_le(data, 60)? as usize;
        let type_ids_size = read_u32_le(data, 64)? as usize;
        let type_ids_off = read_u32_le(data, 68)? as usize;
        let class_defs_size = read_u32_le(data, 96)? as usize;
        let class_defs_off = read_u32_le(data, 100)? as usize;

        // 1. Parse string_ids -> Vec<String>
        let mut strings = Vec::with_capacity(string_ids_size.min(65536));
        for i in 0..string_ids_size {
            let entry_off = string_ids_off
                .checked_add(i.checked_mul(4).ok_or_else(|| IkazuchiError::Parse("Overflow".into()))?)
                .ok_or_else(|| IkazuchiError::Parse("Overflow".into()))?;
            let str_data_off = read_u32_le(data, entry_off)? as usize;
            let s = read_dex_string(data, str_data_off)?;
            strings.push(s);
        }

        // 2. Parse type_ids -> Vec<String>
        let mut types = Vec::with_capacity(type_ids_size.min(65536));
        for i in 0..type_ids_size {
            let entry_off = type_ids_off
                .checked_add(i.checked_mul(4).ok_or_else(|| IkazuchiError::Parse("Overflow".into()))?)
                .ok_or_else(|| IkazuchiError::Parse("Overflow".into()))?;
            let descriptor_idx = read_u32_le(data, entry_off)? as usize;
            let desc = strings
                .get(descriptor_idx)
                .cloned()
                .ok_or_else(|| IkazuchiError::Parse(format!("Invalid type descriptor_idx {descriptor_idx}")))?;
            types.push(desc);
        }

        // 3. Parse class_defs (each entry is 32 bytes)
        let mut classes = Vec::with_capacity(class_defs_size.min(16384));
        for i in 0..class_defs_size {
            let entry_off = class_defs_off
                .checked_add(i.checked_mul(32).ok_or_else(|| IkazuchiError::Parse("Overflow".into()))?)
                .ok_or_else(|| IkazuchiError::Parse("Overflow".into()))?;

            if entry_off + 32 > data.len() {
                return Err(IkazuchiError::Parse("class_defs entry out of bounds".into()));
            }

            let class_idx = read_u32_le(data, entry_off)?;
            let access_flags = read_u32_le(data, entry_off + 4)?;
            let superclass_idx = read_u32_le(data, entry_off + 8)?;
            let source_file_idx = read_u32_le(data, entry_off + 16)?;

            let class_descriptor = types
                .get(class_idx as usize)
                .cloned()
                .ok_or_else(|| IkazuchiError::Parse(format!("Invalid class_idx {class_idx}")))?;

            let superclass_descriptor = if superclass_idx == NO_INDEX {
                None
            } else {
                types.get(superclass_idx as usize).cloned()
            };

            let source_file = if source_file_idx == NO_INDEX {
                None
            } else {
                strings.get(source_file_idx as usize).cloned()
            };

            classes.push(DexClassDef {
                class_descriptor,
                superclass_descriptor,
                access_flags,
                source_file,
            });
        }

        Ok(Self {
            version,
            strings,
            types,
            classes,
        })
    }

    /// Returns all HTTP/HTTPS URLs present in the DEX string table (excluding Android XML schemas).
    pub fn extract_http_urls(&self) -> Vec<String> {
        let mut urls: Vec<String> = self
            .strings
            .iter()
            .filter(|s| {
                (s.starts_with("https://") || s.starts_with("http://"))
                    && s.len() > 10
                    && !s.contains("schemas.android.com")
                    && !s.contains("www.w3.org")
            })
            .cloned()
            .collect();
        urls.sort();
        urls.dedup();
        urls
    }
}

fn read_u32_le(data: &[u8], offset: usize) -> Result<u32> {
    let slice = data
        .get(offset..offset + 4)
        .ok_or_else(|| IkazuchiError::Parse(format!("Unexpected EOF reading u32 at offset {offset}")))?;
    Ok(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

/// Reads a ULEB128-prefixed MUTF-8 string at `offset` in the DEX buffer.
fn read_dex_string(data: &[u8], offset: usize) -> Result<String> {
    if offset >= data.len() {
        return Err(IkazuchiError::Parse(format!(
            "String data offset {offset} out of bounds"
        )));
    }
    let (_utf16_len, bytes_read) = read_uleb128(&data[offset..])?;
    let str_start = offset + bytes_read;
    let remaining = data
        .get(str_start..)
        .ok_or_else(|| IkazuchiError::Parse("String start out of bounds".into()))?;
    let nul_pos = remaining.iter().position(|&b| b == 0).unwrap_or(remaining.len());
    Ok(String::from_utf8_lossy(&remaining[..nul_pos]).to_string())
}

/// Decodes an unsigned LEB128 integer, returning `(value, bytes_consumed)`.
fn read_uleb128(slice: &[u8]) -> Result<(u32, usize)> {
    let mut result: u32 = 0;
    let mut shift = 0;
    for (idx, &byte) in slice.iter().take(5).enumerate() {
        result |= u32::from(byte & 0x7F) << shift;
        if (byte & 0x80) == 0 {
            return Ok((result, idx + 1));
        }
        shift += 7;
    }
    Err(IkazuchiError::Parse("Malformed ULEB128 in DEX string".into()))
}

/// Builds a valid minimal `classes.dex` binary for testing and verification.
pub fn build_synthetic_dex(strings: &[&str], type_string_indices: &[u32], class_defs: &[(u32, u32, u32)]) -> Vec<u8> {
    let mut buf = vec![0u8; 112];
    buf[0..8].copy_from_slice(b"dex\n035\0");

    // 1. string_ids table starts at 112
    let string_ids_off = buf.len() as u32;
    let string_ids_size = strings.len() as u32;
    buf.resize(buf.len() + strings.len() * 4, 0);

    // 2. type_ids table
    let type_ids_off = buf.len() as u32;
    let type_ids_size = type_string_indices.len() as u32;
    for &str_idx in type_string_indices {
        buf.extend_from_slice(&str_idx.to_le_bytes());
    }

    // 3. class_defs table (32 bytes each)
    let class_defs_off = buf.len() as u32;
    let class_defs_size = class_defs.len() as u32;
    for &(class_idx, superclass_idx, source_file_idx) in class_defs {
        let mut entry = [0u8; 32];
        entry[0..4].copy_from_slice(&class_idx.to_le_bytes());
        entry[4..8].copy_from_slice(&1u32.to_le_bytes()); // PUBLIC
        entry[8..12].copy_from_slice(&superclass_idx.to_le_bytes());
        entry[16..20].copy_from_slice(&source_file_idx.to_le_bytes());
        buf.extend_from_slice(&entry);
    }

    // 4. Write string data payloads and backfill string_ids_off
    for (i, &s) in strings.iter().enumerate() {
        let data_off = buf.len() as u32;
        let id_pos = (string_ids_off as usize) + i * 4;
        buf[id_pos..id_pos + 4].copy_from_slice(&data_off.to_le_bytes());

        // Write ULEB128 length (for ASCII < 128, 1 byte)
        let bytes = s.as_bytes();
        let mut len = bytes.len();
        loop {
            let mut b = (len & 0x7F) as u8;
            len >>= 7;
            if len != 0 {
                b |= 0x80;
            }
            buf.push(b);
            if len == 0 {
                break;
            }
        }
        buf.extend_from_slice(bytes);
        buf.push(0); // NUL terminator
    }

    let file_size = buf.len() as u32;
    buf[32..36].copy_from_slice(&file_size.to_le_bytes());
    buf[36..40].copy_from_slice(&112u32.to_le_bytes());
    buf[56..60].copy_from_slice(&string_ids_size.to_le_bytes());
    buf[60..64].copy_from_slice(&string_ids_off.to_le_bytes());
    buf[64..68].copy_from_slice(&type_ids_size.to_le_bytes());
    buf[68..72].copy_from_slice(&type_ids_off.to_le_bytes());
    buf[96..100].copy_from_slice(&class_defs_size.to_le_bytes());
    buf[100..104].copy_from_slice(&class_defs_off.to_le_bytes());

    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dex_parser_decodes_class_hierarchy_and_urls() {
        let strings = [
            "AsuraScans.kt",
            "Leu/kanade/tachiyomi/extension/en/asurascans/AsuraScans;",
            "Leu/kanade/tachiyomi/multisrc/mangathemesia/MangaThemesia;",
            "https://asuracomic.net",
            "Asura Scans",
            "en",
        ];
        // type 0 -> string 1 (AsuraScans), type 1 -> string 2 (MangaThemesia)
        let types = [1u32, 2u32];
        // class 0: class_idx = 0, superclass_idx = 1, source_file_idx = 0
        let classes = [(0u32, 1u32, 0u32)];

        let dex_bytes = build_synthetic_dex(&strings, &types, &classes);
        let parsed = DexFile::parse(&dex_bytes).expect("Should parse synthetic DEX");

        assert_eq!(parsed.version, "035");
        assert_eq!(parsed.classes.len(), 1);
        assert_eq!(
            parsed.classes[0].class_descriptor,
            "Leu/kanade/tachiyomi/extension/en/asurascans/AsuraScans;"
        );
        assert_eq!(
            parsed.classes[0].superclass_descriptor.as_deref(),
            Some("Leu/kanade/tachiyomi/multisrc/mangathemesia/MangaThemesia;")
        );
        assert_eq!(parsed.extract_http_urls(), vec!["https://asuracomic.net".to_string()]);
    }
}
