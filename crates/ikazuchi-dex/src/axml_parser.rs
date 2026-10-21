//! # Pure-Rust `AndroidManifest.xml` Metadata Extractor
//!
//! Extracts Mihon extension metadata (`package`, `tachiyomi.extension.class`,
//! `tachiyomi.extension.nsfw`, `tachiyomix.extensionLib`) from both plain-text XML
//! and Android Binary XML (`AXML` magic `0x00080003`) inside `.apk` archives.

use ikazuchi_core::{IkazuchiError, Result};
use serde::{Deserialize, Serialize};

/// Extracted Mihon extension manifest metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ExtensionManifest {
    pub package_name: String,
    pub source_classes: Vec<String>,
    pub nsfw: bool,
    pub lib_version: String,
}

impl ExtensionManifest {
    /// Resolves relative class names (like `".AsuraScans"`) against `package_name`
    /// (just like Mihon's `ExtensionLoader.kt` lines 407–411).
    pub fn resolved_source_classes(&self) -> Vec<String> {
        self.source_classes
            .iter()
            .map(|cls| {
                let trimmed = cls.trim();
                if trimmed.starts_with('.') {
                    format!("{}{}", self.package_name, trimmed)
                } else {
                    trimmed.to_string()
                }
            })
            .collect()
    }
}

/// Parses `AndroidManifest.xml` bytes (supporting both plain UTF-8 XML and Android binary AXML).
pub fn parse_android_manifest(bytes: &[u8]) -> Result<ExtensionManifest> {
    if bytes.len() < 4 {
        return Err(IkazuchiError::Parse("AndroidManifest.xml is empty".into()));
    }

    // Check if plain UTF-8 XML (starts with '<' or whitespace)
    let first_non_ws = bytes.iter().copied().find(|b| !b.is_ascii_whitespace());
    if first_non_ws == Some(b'<') {
        let text = String::from_utf8_lossy(bytes);
        return parse_plain_xml_manifest(&text);
    }

    // Otherwise parse Android Binary XML (AXML magic 0x00080003)
    parse_binary_axml_manifest(bytes)
}

fn parse_plain_xml_manifest(xml: &str) -> Result<ExtensionManifest> {
    let package_name = extract_xml_attr(xml, "package").unwrap_or_default();
    let mut source_classes = Vec::new();
    let mut nsfw = false;
    let mut lib_version = "1.4".to_string();

    for line in xml.lines() {
        if line.contains("tachiyomi.extension.class") {
            if let Some(val) = extract_xml_attr(line, "android:value").or_else(|| extract_xml_attr(line, "value")) {
                source_classes = val
                    .split(';')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
            }
        } else if line.contains("tachiyomi.extension.nsfw") {
            if let Some(val) = extract_xml_attr(line, "android:value").or_else(|| extract_xml_attr(line, "value")) {
                nsfw = val.trim() == "1" || val.trim().eq_ignore_ascii_case("true");
            }
        } else if line.contains("tachiyomix.extensionLib") {
            if let Some(val) = extract_xml_attr(line, "android:value").or_else(|| extract_xml_attr(line, "value")) {
                lib_version = val.trim().to_string();
            }
        }
    }

    Ok(ExtensionManifest {
        package_name,
        source_classes,
        nsfw,
        lib_version,
    })
}

fn extract_xml_attr(text: &str, attr_name: &str) -> Option<String> {
    let pattern = format!("{attr_name}=\"");
    let start = text.find(&pattern)? + pattern.len();
    let rest = &text[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// Parses Android Binary XML (`RES_XML_TYPE = 0x0003`, `RES_STRING_POOL_TYPE = 0x0001`).
fn parse_binary_axml_manifest(bytes: &[u8]) -> Result<ExtensionManifest> {
    let magic = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    if magic != 0x0008_0003 && (magic & 0xFFFF) != 0x0003 {
        return Err(IkazuchiError::Parse(format!(
            "Invalid AXML magic header: 0x{magic:08x}"
        )));
    }

    // Extract UTF-8 and UTF-16LE strings from the binary string pool
    let strings = extract_axml_strings(bytes);
    let mut manifest = ExtensionManifest {
        lib_version: "1.4".to_string(),
        ..Default::default()
    };

    for (idx, s) in strings.iter().enumerate() {
        if s.starts_with("eu.kanade.tachiyomi.extension.") && manifest.package_name.is_empty() {
            manifest.package_name = s.clone();
        }
        if s == "tachiyomi.extension.class" {
            // The class name string in the pool is typically adjacent
            if let Some(next_str) = strings.get(idx + 1) {
                if next_str.starts_with('.') || next_str.contains('.') {
                    manifest.source_classes = next_str
                        .split(';')
                        .map(|c| c.trim().to_string())
                        .filter(|c| !c.is_empty())
                        .collect();
                }
            }
        }
    }

    // Fallback: look for any string starting with `.` that looks like a Kotlin class name
    if manifest.source_classes.is_empty() {
        for s in &strings {
            if s.starts_with('.')
                && s.len() > 2
                && s.chars().nth(1).is_some_and(|c| c.is_ascii_uppercase())
                && !s.contains('/')
            {
                manifest.source_classes.push(s.clone());
                break;
            }
        }
    }

    Ok(manifest)
}

fn extract_axml_strings(bytes: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    // 1. Scan for UTF-16LE strings (standard in AXML string pools)
    let mut i = 0;
    while i + 4 < bytes.len() {
        // Check for a run of printable ASCII chars interleaved with 0x00
        if bytes[i].is_ascii_graphic() && bytes[i + 1] == 0 && bytes[i + 2].is_ascii_graphic() && bytes[i + 3] == 0 {
            let mut chars = Vec::new();
            while i + 1 < bytes.len() && (bytes[i].is_ascii_graphic() || bytes[i] == b' ') && bytes[i + 1] == 0 {
                chars.push(bytes[i]);
                i += 2;
            }
            if chars.len() >= 2 {
                if let Ok(s) = String::from_utf8(chars) {
                    out.push(s);
                }
            }
        } else {
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plain_xml_manifest_parsing() {
        let xml = r#"
        <manifest xmlns:android="http://schemas.android.com/apk/res/android"
            package="eu.kanade.tachiyomi.extension.en.asurascans">
            <uses-feature android:name="tachiyomi.extension" />
            <application>
                <meta-data android:name="tachiyomi.extension.class" android:value=".AsuraScans" />
                <meta-data android:name="tachiyomi.extension.nsfw" android:value="0" />
                <meta-data android:name="tachiyomix.extensionLib" android:value="1.4" />
            </application>
        </manifest>
        "#;

        let manifest = parse_android_manifest(xml.as_bytes()).unwrap();
        assert_eq!(manifest.package_name, "eu.kanade.tachiyomi.extension.en.asurascans");
        assert_eq!(manifest.source_classes, vec![".AsuraScans"]);
        assert_eq!(
            manifest.resolved_source_classes(),
            vec!["eu.kanade.tachiyomi.extension.en.asurascans.AsuraScans"]
        );
        assert!(!manifest.nsfw);
        assert_eq!(manifest.lib_version, "1.4");
    }
}
