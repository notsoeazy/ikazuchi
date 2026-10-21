//! # `ikazuchi-dex`
//!
//! Universal Extension Engine for Ikazuchi:
//! 1. Parses Mihon Extension Repository indices (`index.min.json`).
//! 2. Unpacks `.apk` archives and parses `AndroidManifest.xml` (`axml_parser`) + `classes.dex` (`dex_parser`) in pure Rust.
//! 3. Detects Multisrc themes (`Madara`, `MangaThemesia`, `MangaStream`, `Mangabox`, `MangaDex`) from DEX class hierarchies.
//! 4. Instantiates native Rust `MangaSource` drivers (`drivers/`) via `ExtensionManager`.

pub mod axml_parser;
pub mod chapter_num;
pub mod dex_parser;
pub mod drivers;
pub mod manager;

use ikazuchi_core::{ExtensionSettings, IkazuchiError, Result};
use serde::{Deserialize, Serialize};

pub use axml_parser::{ExtensionManifest, parse_android_manifest};
pub use chapter_num::parse_chapter_number;
pub use dex_parser::{DexClassDef, DexFile, build_synthetic_dex};
pub use drivers::{
    local::{LOCAL_SOURCE_ID, LocalSource},
    madara::MadaraSource,
    mangabox::MangaboxSource,
    mangadex::MangaDexSource,
    mangathemesia::MangaThemesiaSource,
};
pub use manager::{
    ExtensionManager, GlobalSearchResult, LoadedExtension, detect_theme_from_dex,
    unpack_and_analyze_apk,
};

/// Represents an extension entry in a Mihon v1 (`index.min.json`) or Keiyoushi v2 (`index.json`) repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoExtensionEntry {
    pub name: String,
    pub pkg: String,
    pub apk: String,
    pub lang: String,
    #[serde(deserialize_with = "deserialize_string_or_i64")]
    pub code: i64,
    pub version: String,
    #[serde(default)]
    pub nsfw: i32,
    #[serde(default)]
    pub sources: Vec<RepoSourceEntry>,
    /// Optional direct icon URL (populated by Keiyoushi v2 `resources.iconUrl`).
    #[serde(default)]
    pub custom_icon_url: Option<String>,
    /// Optional pre-compiled desktop `.jar` URL (populated by Keiyoushi v2 `resources.jarUrl`).
    #[serde(default)]
    pub jar_url: Option<String>,
}

impl RepoExtensionEntry {
    /// Returns the display name with the `"Tachiyomi: "` prefix stripped (matching Mihon's `NetworkLegacyExtension.kt`).
    pub fn clean_name(&self) -> &str {
        self.name.strip_prefix("Tachiyomi: ").unwrap_or(&self.name)
    }

    /// Computes the full APK download URL from the repository's base URL (or returns `self.apk` directly
    /// if it is already an absolute URL, as in Keiyoushi v2 GitHub Releases).
    pub fn apk_download_url(&self, store_base_url: &str) -> String {
        if self.apk.starts_with("http://") || self.apk.starts_with("https://") {
            return self.apk.clone();
        }
        let base = store_base_url
            .trim_end_matches('/')
            .trim_end_matches("/index.min.json")
            .trim_end_matches("/index.json")
            .trim_end_matches("/index.pb")
            .trim_end_matches("/repo.json");
        let apk_rel = self.apk.trim_start_matches('/').trim_start_matches("apk/");
        format!("{base}/apk/{apk_rel}")
    }

    /// Computes the extension icon URL from the repository's base URL (or returns `custom_icon_url`
    /// if provided by Keiyoushi v2).
    pub fn icon_url(&self, store_base_url: &str) -> String {
        if let Some(icon) = &self.custom_icon_url {
            if icon.starts_with("http://") || icon.starts_with("https://") {
                return icon.clone();
            }
        }
        let base = store_base_url
            .trim_end_matches('/')
            .trim_end_matches("/index.min.json")
            .trim_end_matches("/index.json")
            .trim_end_matches("/index.pb")
            .trim_end_matches("/repo.json");
        format!("{base}/icon/{}.png", self.pkg)
    }

    /// Returns `true` if this entry is a dummy deprecation notice injected into legacy `index.min.json`
    /// (`"Outdated App"` / `"Update to Mihon 0.20.1+"`).
    pub fn is_deprecation_stub(&self) -> bool {
        matches!(
            self.pkg.as_str(),
            "eu.kanade.tachiyomi.extension.all.keiyoushi"
                | "eu.kanade.tachiyomi.extension.all.mihon"
        ) || self.name == "Outdated App"
            || self.name.starts_with("Update to Mihon")
    }

    /// Returns `true` if this repository extension passes the user's [`ExtensionSettings`]
    /// (NSFW filter and enabled language filter).
    pub fn matches_settings(&self, settings: &ExtensionSettings) -> bool {
        if !settings.show_nsfw && self.nsfw != 0 {
            return false;
        }
        if settings.enabled_languages.is_empty() {
            return true;
        }
        if settings.is_language_enabled(&self.lang) {
            return true;
        }
        self.sources
            .iter()
            .any(|s| settings.is_language_enabled(&s.lang))
    }
}

/// Filters a list of remote repository extensions (`index.json` / `index.min.json`) according to
/// the user's [`ExtensionSettings`] (hiding NSFW extensions when `show_nsfw == false`, filtering
/// by `enabled_languages`, and removing disabled sub-sources).
pub fn filter_repo_extensions(
    entries: &[RepoExtensionEntry],
    settings: &ExtensionSettings,
) -> Vec<RepoExtensionEntry> {
    let mut filtered = Vec::new();
    for entry in entries {
        if !entry.matches_settings(settings) {
            continue;
        }
        let mut cloned = entry.clone();
        if !cloned.sources.is_empty() {
            cloned.sources.retain(|src| {
                !settings.is_source_disabled(src.id)
                    && (settings.enabled_languages.is_empty()
                        || settings.is_language_enabled(&src.lang)
                        || settings.is_language_enabled(&entry.lang))
            });
            if cloned.sources.is_empty() {
                continue;
            }
        }
        filtered.push(cloned);
    }
    filtered
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoSourceEntry {
    pub name: String,
    #[serde(alias = "language")]
    pub lang: String,
    #[serde(deserialize_with = "deserialize_string_or_i64")]
    pub id: i64,
    #[serde(rename = "baseUrl", alias = "homeUrl", default)]
    pub base_url: String,
    #[serde(rename = "mirrorUrls", default)]
    pub mirror_urls: Vec<String>,
}

fn deserialize_string_or_i64<'de, D>(deserializer: D) -> std::result::Result<i64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum StringOrInt {
        Str(String),
        Int(i64),
    }
    match StringOrInt::deserialize(deserializer)? {
        StringOrInt::Str(s) => s.parse::<i64>().map_err(serde::de::Error::custom),
        StringOrInt::Int(i) => Ok(i),
    }
}

/// Detected Multisrc theme family from a Mihon `.apk`'s `classes.dex`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MultisrcTheme {
    Madara,
    MangaThemesia,
    MangaStream,
    Mangabox,
    Keyoapp,
    ZeistManga,
    MangaDex,
    CustomBytecode,
}

/// Metadata extracted directly from a Mihon `.apk`'s `classes.dex` in pure Rust.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InspectedApkMetadata {
    pub detected_theme: MultisrcTheme,
    pub extracted_urls: Vec<String>,
    pub dex_size_bytes: usize,
}

/// Keiyoushi / Mihon 0.20.1+ v2 `NetworkExtensionStore` JSON format (`index.json`).
#[derive(Debug, Clone, Deserialize)]
struct V2NetworkExtensionStore {
    #[serde(rename = "extensionList")]
    extension_list: Option<V2ExtensionList>,
    #[serde(default)]
    extensions: Vec<V2Extension>,
}

#[derive(Debug, Clone, Deserialize)]
struct V2ExtensionList {
    extensions: Vec<V2Extension>,
}

#[derive(Debug, Clone, Deserialize)]
struct V2Extension {
    name: String,
    #[serde(rename = "packageName")]
    package_name: String,
    resources: V2Resources,
    #[serde(rename = "versionCode", deserialize_with = "deserialize_string_or_i64")]
    version_code: i64,
    #[serde(rename = "versionName")]
    version_name: String,
    #[serde(rename = "contentWarning", default)]
    content_warning: String,
    #[serde(default)]
    sources: Vec<RepoSourceEntry>,
}

#[derive(Debug, Clone, Deserialize)]
struct V2Resources {
    #[serde(rename = "apkUrl")]
    apk_url: String,
    #[serde(rename = "iconUrl", default)]
    icon_url: Option<String>,
    #[serde(rename = "jarUrl", default)]
    jar_url: Option<String>,
}

impl From<V2Extension> for RepoExtensionEntry {
    fn from(v2: V2Extension) -> Self {
        let mut unique_langs: Vec<&str> = v2.sources.iter().map(|s| s.lang.as_str()).collect();
        unique_langs.sort_unstable();
        unique_langs.dedup();
        let lang = if unique_langs.len() == 1 {
            unique_langs[0].to_string()
        } else {
            "all".to_string()
        };
        let nsfw = i32::from(
            v2.content_warning.contains("NSFW") || v2.content_warning.contains("MIXED"),
        );

        RepoExtensionEntry {
            name: v2.name,
            pkg: v2.package_name,
            apk: v2.resources.apk_url,
            lang,
            code: v2.version_code,
            version: v2.version_name,
            nsfw,
            sources: v2.sources,
            custom_icon_url: v2.resources.icon_url,
            jar_url: v2.resources.jar_url,
        }
    }
}

/// Normalizes a Mihon / Keiyoushi / Yokai repository URL:
/// 1. Strips query parameters (`?`) and URL fragments (`#`).
/// 2. Rewrites `https://github.com/{owner}/{repo}/raw/{ref}/{path}` -> `https://raw.githubusercontent.com/{owner}/{repo}/{ref}/{path}`.
/// 3. Automatically upgrades legacy `keiyoushi/extensions/repo/index.min.json` or `/repo.json` URLs to `/index.json`.
/// 4. Appends `/index.json` if the user entered a bare repository base URL (e.g. `.../keiyoushi/extensions/repo`).
pub fn normalize_mihon_repo_url(url: &str) -> String {
    let mut cleaned = url
        .trim()
        .split('?')
        .next()
        .unwrap_or("")
        .split('#')
        .next()
        .unwrap_or("")
        .trim_end_matches('/')
        .to_string();

    if let Some(rest) = cleaned.strip_prefix("https://github.com/") {
        let parts: Vec<&str> = rest.splitn(5, '/').collect();
        if parts.len() == 5 && parts[2] == "raw" {
            cleaned = format!(
                "https://raw.githubusercontent.com/{}/{}/{}/{}",
                parts[0], parts[1], parts[3], parts[4]
            );
        }
    }

    if cleaned.contains("keiyoushi/extensions") && cleaned.ends_with("/index.min.json") {
        cleaned.replace("/index.min.json", "/index.json")
    } else if cleaned.ends_with("/repo.json") {
        cleaned.replace("/repo.json", "/index.json")
    } else if !cleaned.ends_with(".json") && !cleaned.ends_with(".pb") && !cleaned.is_empty() {
        format!("{cleaned}/index.json")
    } else {
        cleaned
    }
}

/// Parses a binary Protobuf `index.pb` extension repository payload (`ExtensionIndexDto` in Yokai / Mihon).
pub fn parse_mihon_repo_index_pb(bytes: &[u8]) -> Result<Vec<RepoExtensionEntry>> {
    let mut pos = 0usize;
    let mut entries = Vec::new();

    while pos < bytes.len() {
        let (field_num, wire_type) = pb_read_tag(bytes, &mut pos)?;
        match (field_num, wire_type) {
            (101, 2) => {
                let list_bytes = pb_read_bytes(bytes, &mut pos)?;
                let mut lpos = 0usize;
                while lpos < list_bytes.len() {
                    let (lf, lwt) = pb_read_tag(list_bytes, &mut lpos)?;
                    if lf == 1 && lwt == 2 {
                        let ext_bytes = pb_read_bytes(list_bytes, &mut lpos)?;
                        let entry = decode_pb_extension(ext_bytes)?;
                        if !entry.is_deprecation_stub() {
                            entries.push(entry);
                        }
                    } else {
                        pb_skip_field(list_bytes, &mut lpos, lwt)?;
                    }
                }
            }
            _ => pb_skip_field(bytes, &mut pos, wire_type)?,
        }
    }

    Ok(entries)
}

/// Encodes a slice of [`RepoExtensionEntry`] into a Mihon/Yokai `index.pb` (`ExtensionIndexDto`) binary Protobuf payload.
pub fn encode_mihon_repo_index_pb(entries: &[RepoExtensionEntry]) -> Vec<u8> {
    let mut list_buf = Vec::new();
    for e in entries {
        let mut ext_buf = Vec::new();
        pb_write_string(&mut ext_buf, 1, &e.name);
        pb_write_string(&mut ext_buf, 2, &e.pkg);

        let mut res_buf = Vec::new();
        pb_write_string(&mut res_buf, 1, &e.apk);
        if let Some(icon) = &e.custom_icon_url {
            pb_write_string(&mut res_buf, 2, icon);
        }
        if let Some(jar) = &e.jar_url {
            pb_write_string(&mut res_buf, 3, jar);
        }
        pb_write_bytes(&mut ext_buf, 3, &res_buf);

        pb_write_string(&mut ext_buf, 4, "1.4");
        pb_write_varint_field(&mut ext_buf, 5, e.code as u64);
        pb_write_string(&mut ext_buf, 6, &e.version);
        // ContentWarning: 1 = SAFE, 3 = NSFW
        pb_write_varint_field(&mut ext_buf, 7, if e.nsfw != 0 { 3 } else { 1 });

        for s in &e.sources {
            let mut src_buf = Vec::new();
            pb_write_varint_field(&mut src_buf, 1, s.id as u64);
            pb_write_string(&mut src_buf, 2, &s.name);
            pb_write_string(&mut src_buf, 3, &s.lang);
            pb_write_string(&mut src_buf, 4, &s.base_url);
            pb_write_bytes(&mut ext_buf, 8, &src_buf);
        }

        pb_write_bytes(&mut list_buf, 1, &ext_buf);
    }

    let mut root_buf = Vec::new();
    pb_write_string(&mut root_buf, 1, "Keiyoushi");
    pb_write_bytes(&mut root_buf, 101, &list_buf);
    root_buf
}

fn decode_pb_extension(bytes: &[u8]) -> Result<RepoExtensionEntry> {
    let mut pos = 0usize;
    let mut name = String::new();
    let mut pkg = String::new();
    let mut apk = String::new();
    let mut custom_icon_url = None;
    let mut jar_url = None;
    let mut code = 0i64;
    let mut version = String::new();
    let mut content_warning = 0u64;
    let mut sources = Vec::new();

    while pos < bytes.len() {
        let (field, wt) = pb_read_tag(bytes, &mut pos)?;
        match (field, wt) {
            (1, 2) => name = pb_read_string(bytes, &mut pos)?,
            (2, 2) => pkg = pb_read_string(bytes, &mut pos)?,
            (3, 2) => {
                let rbytes = pb_read_bytes(bytes, &mut pos)?;
                let mut rpos = 0usize;
                while rpos < rbytes.len() {
                    let (rf, rwt) = pb_read_tag(rbytes, &mut rpos)?;
                    match (rf, rwt) {
                        (1, 2) => apk = pb_read_string(rbytes, &mut rpos)?,
                        (2, 2) => {
                            let icon = pb_read_string(rbytes, &mut rpos)?;
                            if !icon.is_empty() {
                                custom_icon_url = Some(icon);
                            }
                        }
                        (3, 2) => {
                            let jar = pb_read_string(rbytes, &mut rpos)?;
                            if !jar.is_empty() {
                                jar_url = Some(jar);
                            }
                        }
                        _ => pb_skip_field(rbytes, &mut rpos, rwt)?,
                    }
                }
            }
            (5, 0) => code = pb_read_varint(bytes, &mut pos)? as i64,
            (6, 2) => version = pb_read_string(bytes, &mut pos)?,
            (7, 0) => content_warning = pb_read_varint(bytes, &mut pos)?,
            (8, 2) => {
                let sbytes = pb_read_bytes(bytes, &mut pos)?;
                let mut spos = 0usize;
                let mut sid = 0i64;
                let mut sname = String::new();
                let mut slang = String::new();
                let mut sbase = String::new();
                while spos < sbytes.len() {
                    let (sf, swt) = pb_read_tag(sbytes, &mut spos)?;
                    match (sf, swt) {
                        (1, 0) => sid = pb_read_varint(sbytes, &mut spos)? as i64,
                        (2, 2) => sname = pb_read_string(sbytes, &mut spos)?,
                        (3, 2) => slang = pb_read_string(sbytes, &mut spos)?,
                        (4, 2) => sbase = pb_read_string(sbytes, &mut spos)?,
                        _ => pb_skip_field(sbytes, &mut spos, swt)?,
                    }
                }
                sources.push(RepoSourceEntry {
                    id: sid,
                    name: sname,
                    lang: slang,
                    base_url: sbase,
                    mirror_urls: Vec::new(),
                });
            }
            _ => pb_skip_field(bytes, &mut pos, wt)?,
        }
    }

    let mut unique_langs: Vec<&str> = sources.iter().map(|s| s.lang.as_str()).collect();
    unique_langs.sort_unstable();
    unique_langs.dedup();
    let lang = if unique_langs.len() == 1 {
        unique_langs[0].to_string()
    } else {
        "all".to_string()
    };

    Ok(RepoExtensionEntry {
        name,
        pkg,
        apk,
        lang,
        code,
        version,
        // In ExtensionIndexDto.ContentWarning: 2 = MIXED, 3 = NSFW
        nsfw: i32::from(content_warning >= 2),
        sources,
        custom_icon_url,
        jar_url,
    })
}

fn pb_read_varint(buf: &[u8], pos: &mut usize) -> Result<u64> {
    let mut res = 0u64;
    let mut shift = 0;
    while shift < 64 {
        if *pos >= buf.len() {
            return Err(IkazuchiError::Parse("Unexpected EOF in index.pb varint".into()));
        }
        let b = buf[*pos];
        *pos += 1;
        res |= ((b & 0x7F) as u64) << shift;
        if (b & 0x80) == 0 {
            return Ok(res);
        }
        shift += 7;
    }
    Err(IkazuchiError::Parse("Malformed varint in index.pb".into()))
}

fn pb_read_tag(buf: &[u8], pos: &mut usize) -> Result<(u32, u8)> {
    let v = pb_read_varint(buf, pos)?;
    Ok(((v >> 3) as u32, (v & 0x07) as u8))
}

fn pb_read_bytes<'a>(buf: &'a [u8], pos: &mut usize) -> Result<&'a [u8]> {
    let len = pb_read_varint(buf, pos)? as usize;
    if pos.saturating_add(len) > buf.len() {
        return Err(IkazuchiError::Parse("index.pb field exceeds buffer".into()));
    }
    let slice = &buf[*pos..*pos + len];
    *pos += len;
    Ok(slice)
}

fn pb_read_string(buf: &[u8], pos: &mut usize) -> Result<String> {
    let b = pb_read_bytes(buf, pos)?;
    Ok(String::from_utf8_lossy(b).into_owned())
}

fn pb_skip_field(buf: &[u8], pos: &mut usize, wt: u8) -> Result<()> {
    match wt {
        0 => {
            pb_read_varint(buf, pos)?;
            Ok(())
        }
        1 => {
            *pos = pos.saturating_add(8).min(buf.len());
            Ok(())
        }
        2 => {
            pb_read_bytes(buf, pos)?;
            Ok(())
        }
        5 => {
            *pos = pos.saturating_add(4).min(buf.len());
            Ok(())
        }
        other => Err(IkazuchiError::Parse(format!("Unsupported wire type {other}"))),
    }
}

fn pb_write_varint(out: &mut Vec<u8>, mut val: u64) {
    loop {
        if (val & !0x7F) == 0 {
            out.push(val as u8);
            break;
        } else {
            out.push(((val & 0x7F) as u8) | 0x80);
            val >>= 7;
        }
    }
}

fn pb_write_varint_field(out: &mut Vec<u8>, field: u32, val: u64) {
    pb_write_varint(out, (field as u64) << 3);
    pb_write_varint(out, val);
}

fn pb_write_bytes(out: &mut Vec<u8>, field: u32, bytes: &[u8]) {
    pb_write_varint(out, ((field as u64) << 3) | 2);
    pb_write_varint(out, bytes.len() as u64);
    out.extend_from_slice(bytes);
}

fn pb_write_string(out: &mut Vec<u8>, field: u32, s: &str) {
    pb_write_bytes(out, field, s.as_bytes());
}

/// Parses either a Legacy v1 (`index.min.json` array `[...]`) OR a Mihon 0.20.1+ / Keiyoushi v2
/// (`index.json` object `{"extensionList": {"extensions": [...]}}`) payload, filtering out
/// `"Outdated App"` / `"Update to Mihon 0.20.1+"` deprecation stubs.
pub fn parse_mihon_repo_index_json(json_str: &str) -> Result<Vec<RepoExtensionEntry>> {
    let trimmed = json_str.trim();
    let entries: Vec<RepoExtensionEntry> = if trimmed.starts_with('[') {
        serde_json::from_str::<Vec<RepoExtensionEntry>>(trimmed)
            .map_err(|e| IkazuchiError::Parse(e.to_string()))?
    } else if trimmed.starts_with('{') {
        let store: V2NetworkExtensionStore = serde_json::from_str(trimmed)
            .map_err(|e| IkazuchiError::Parse(format!("Invalid v2 extension store JSON: {e}")))?;
        let raw_list = store
            .extension_list
            .map(|el| el.extensions)
            .unwrap_or(store.extensions);
        raw_list.into_iter().map(RepoExtensionEntry::from).collect()
    } else {
        return Err(IkazuchiError::Parse(
            "Extension repository response is neither a JSON array nor object".into(),
        ));
    };

    Ok(entries
        .into_iter()
        .filter(|e| !e.is_deprecation_stub())
        .collect())
}

/// Fetches and parses a remote Mihon / Keiyoushi / Yokai extension repository (`index.pb`, `index.json`, or `index.min.json`),
/// automatically upgrading `/index.min.json` to `/index.json` if the legacy endpoint returns only
/// deprecation stubs.
pub async fn fetch_mihon_repo_index(index_url: &str) -> Result<Vec<RepoExtensionEntry>> {
    let target_url = normalize_mihon_repo_url(index_url);
    let resp = reqwest::get(&target_url)
        .await
        .map_err(|e| IkazuchiError::Network(e.to_string()))?;

    if target_url.ends_with(".pb") {
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| IkazuchiError::Network(e.to_string()))?;
        return parse_mihon_repo_index_pb(&bytes);
    }

    let text = resp
        .text()
        .await
        .map_err(|e| IkazuchiError::Network(e.to_string()))?;
    let entries = parse_mihon_repo_index_json(&text)?;

    // Safeguard #11: If a third-party mirror returned 0 entries on `/index.min.json` due to the
    // deprecation stub, automatically retry `/index.json`!
    if entries.is_empty() && target_url.ends_with("/index.min.json") {
        let v2_url = target_url.replace("/index.min.json", "/index.json");
        let resp_v2 = reqwest::get(&v2_url)
            .await
            .map_err(|e| IkazuchiError::Network(e.to_string()))?;
        let text_v2 = resp_v2
            .text()
            .await
            .map_err(|e| IkazuchiError::Network(e.to_string()))?;
        return parse_mihon_repo_index_json(&text_v2);
    }

    Ok(entries)
}

/// Inspects raw `.apk` bytes and returns its `InspectedApkMetadata`.
pub fn inspect_apk_bytes(apk_bytes: &[u8]) -> Result<InspectedApkMetadata> {
    let (_manifest, _dex, inspected) = unpack_and_analyze_apk(apk_bytes)?;
    Ok(inspected)
}

/// Builds an in-memory `.apk` (ZIP archive containing `AndroidManifest.xml` and `classes.dex`) for testing.
pub fn build_synthetic_apk(manifest_xml: &str, dex_bytes: &[u8]) -> Vec<u8> {
    use std::io::Write;
    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    let mut buf = Vec::new();
    {
        let mut zip = ZipWriter::new(std::io::Cursor::new(&mut buf));
        let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        zip.start_file("AndroidManifest.xml", opts).unwrap();
        zip.write_all(manifest_xml.as_bytes()).unwrap();
        zip.start_file("classes.dex", opts).unwrap();
        zip.write_all(dex_bytes).unwrap();
        zip.finish().unwrap();
    }
    buf
}

#[cfg(test)]
mod tests {
    use super::*;
    use ikazuchi_core::{MangaStatus, generate_source_id};

    #[test]
    fn test_parse_mihon_repo_index_and_urls() {
        let sample_index = r#"[
            {
                "name": "Tachiyomi: Asura Scans",
                "pkg": "eu.kanade.tachiyomi.extension.en.asurascans",
                "apk": "tachiyomi-en.asurascans-v1.4.45.apk",
                "lang": "en",
                "code": 45,
                "version": "1.4.45",
                "nsfw": 0,
                "sources": [
                    {
                        "name": "Asura Scans",
                        "lang": "en",
                        "id": "7123456789012345678",
                        "baseUrl": "https://asuracomic.net"
                    }
                ]
            }
        ]"#;

        let entries = parse_mihon_repo_index_json(sample_index).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].clean_name(), "Asura Scans");
        assert_eq!(
            entries[0].apk_download_url("https://raw.githubusercontent.com/keiyoushi/extensions/repo/index.min.json"),
            "https://raw.githubusercontent.com/keiyoushi/extensions/repo/apk/tachiyomi-en.asurascans-v1.4.45.apk"
        );
        assert_eq!(entries[0].sources[0].id, 7123456789012345678);
    }

    #[test]
    fn test_keiyoushi_v2_store_and_deprecation_stub_safeguard() {
        // 1. Verify legacy `index.min.json` "Outdated App" / "Update to Mihon 0.20.1+" stubs are filtered out!
        let stub_index = r#"[
            {
                "name": "Outdated App",
                "pkg": "eu.kanade.tachiyomi.extension.all.keiyoushi",
                "apk": "tachiyomi-all.keiyoushi-v1.4.1.apk",
                "lang": "all",
                "code": 1,
                "version": "1.4.1",
                "nsfw": 0,
                "sources": []
            },
            {
                "name": "Update to Mihon 0.20.1+",
                "pkg": "eu.kanade.tachiyomi.extension.all.mihon",
                "apk": "tachiyomi-all.mihon-v1.4.1.apk",
                "lang": "all",
                "code": 1,
                "version": "1.4.1",
                "nsfw": 0,
                "sources": []
            }
        ]"#;
        let stub_parsed = parse_mihon_repo_index_json(stub_index).unwrap();
        assert!(stub_parsed.is_empty());

        // 2. Verify URL auto-upgrade from /index.min.json -> /index.json for Keiyoushi,
        // GitHub /raw/ rewrite, and bare base URL handling (Yokai CreateExtensionRepo parity)
        assert_eq!(
            normalize_mihon_repo_url(
                "https://raw.githubusercontent.com/keiyoushi/extensions/repo/index.min.json"
            ),
            "https://raw.githubusercontent.com/keiyoushi/extensions/repo/index.json"
        );
        assert_eq!(
            normalize_mihon_repo_url(
                "https://github.com/keiyoushi/extensions/raw/repo/index.pb?raw=true"
            ),
            "https://raw.githubusercontent.com/keiyoushi/extensions/repo/index.pb"
        );
        assert_eq!(
            normalize_mihon_repo_url("https://raw.githubusercontent.com/keiyoushi/extensions/repo"),
            "https://raw.githubusercontent.com/keiyoushi/extensions/repo/index.json"
        );

        // 3. Verify real Keiyoushi v2 `NetworkExtensionStore` JSON structure (`index.json`)
        let keiyoushi_v2_json = r#"{
            "name": "Keiyoushi",
            "badgeLabel": "KEI",
            "signingKey": "9add655a78e96c4ec7a53ef89dccb557cb5d767489fac5e785d671a5a75d4da2",
            "contact": {
                "website": "https://keiyoushi.github.io",
                "discord": "https://discord.gg/3FbCpdKbdY"
            },
            "extensionList": {
                "extensions": [
                    {
                        "name": "MangaDex",
                        "packageName": "eu.kanade.tachiyomi.extension.all.mangadex",
                        "resources": {
                            "apkUrl": "https://github.com/keiyoushi/extensions/releases/download/v1.4.205/tachiyomi-all.mangadex-v1.4.205.apk",
                            "iconUrl": "https://cdn.jsdelivr.net/gh/keiyoushi/extensions-source@main/src/all/mangadex/res/mipmap-xhdpi/ic_launcher.png",
                            "jarUrl": "https://github.com/keiyoushi/extensions/releases/download/v1.4.205/tachiyomi-all.mangadex-v1.4.205.jar"
                        },
                        "extensionLib": "1.4",
                        "versionCode": "205",
                        "versionName": "1.4.205",
                        "contentWarning": "CONTENT_WARNING_SAFE",
                        "sources": [
                            {
                                "id": "2499283573021220255",
                                "name": "MangaDex",
                                "language": "en",
                                "homeUrl": "https://mangadex.org"
                            }
                        ]
                    }
                ]
            }
        }"#;

        let v2_entries = parse_mihon_repo_index_json(keiyoushi_v2_json).unwrap();
        assert_eq!(v2_entries.len(), 1);
        assert_eq!(v2_entries[0].name, "MangaDex");
        assert_eq!(v2_entries[0].code, 205);
        assert_eq!(v2_entries[0].lang, "en");
        assert_eq!(v2_entries[0].nsfw, 0);
        assert_eq!(
            v2_entries[0].apk_download_url("https://raw.githubusercontent.com/keiyoushi/extensions/repo/index.json"),
            "https://github.com/keiyoushi/extensions/releases/download/v1.4.205/tachiyomi-all.mangadex-v1.4.205.apk"
        );
        assert_eq!(
            v2_entries[0].jar_url.as_deref(),
            Some("https://github.com/keiyoushi/extensions/releases/download/v1.4.205/tachiyomi-all.mangadex-v1.4.205.jar")
        );
        assert_eq!(v2_entries[0].sources[0].id, 2499283573021220255);
        assert_eq!(v2_entries[0].sources[0].base_url, "https://mangadex.org");

        // 4. Verify Yokai/Mihon `index.pb` binary Protobuf extension store encoding & decoding
        let pb_bytes = encode_mihon_repo_index_pb(&v2_entries);
        let pb_decoded = parse_mihon_repo_index_pb(&pb_bytes).unwrap();
        assert_eq!(pb_decoded.len(), 1);
        assert_eq!(pb_decoded[0].name, "MangaDex");
        assert_eq!(pb_decoded[0].pkg, "eu.kanade.tachiyomi.extension.all.mangadex");
        assert_eq!(pb_decoded[0].code, 205);
        assert_eq!(pb_decoded[0].sources[0].id, 2499283573021220255);
        assert_eq!(pb_decoded[0].sources[0].base_url, "https://mangadex.org");
    }

    #[test]
    fn test_extension_manager_loads_synthetic_apk_and_instantiates_driver() {
        let manifest_xml = r#"
        <manifest package="eu.kanade.tachiyomi.extension.en.asurascans">
            <uses-feature android:name="tachiyomi.extension" />
            <application>
                <meta-data android:name="tachiyomi.extension.class" android:value=".AsuraScans" />
            </application>
        </manifest>
        "#;

        let strings = [
            "AsuraScans.kt",
            "Leu/kanade/tachiyomi/extension/en/asurascans/AsuraScans;",
            "Leu/kanade/tachiyomi/multisrc/mangathemesia/MangaThemesia;",
            "https://asuracomic.net",
        ];
        let dex_bytes = build_synthetic_dex(&strings, &[1, 2], &[(0, 1, 0)]);
        let apk_bytes = build_synthetic_apk(manifest_xml, &dex_bytes);

        let mut mgr = ExtensionManager::new();
        let loaded = mgr.load_apk_from_bytes(&apk_bytes, None).expect("APK should load");

        assert_eq!(loaded.pkg_name, "eu.kanade.tachiyomi.extension.en.asurascans");
        assert_eq!(loaded.theme, MultisrcTheme::MangaThemesia);
        assert_eq!(loaded.sources.len(), 1);

        let src = &loaded.sources[0];
        assert_eq!(src.name(), "AsuraScans");
        assert_eq!(src.lang(), "en");
        assert_eq!(src.base_url(), "https://asuracomic.net");
        assert_eq!(src.id(), generate_source_id("AsuraScans", "en", 1));
    }

    #[test]
    fn test_madara_driver_html_scraping_pipeline() {
        let driver = MadaraSource::new("Leviatan Scans", "https://leviatanscans.com", "en", 1);

        let catalog_html = r#"
        <div class="page-item-detail">
            <div class="item-thumb">
                <img data-src="https://leviatanscans.com/covers/legend-of-northern-blade.jpg" />
            </div>
            <div class="post-title">
                <h3><a href="https://leviatanscans.com/manga/legend-of-the-northern-blade/">Legend of the Northern Blade</a></h3>
            </div>
        </div>
        <div class="nav-previous"><a href="/page/2/">Older Posts</a></div>
        "#;

        let page = driver.parse_catalog_html(catalog_html).unwrap();
        assert!(page.has_next_page);
        assert_eq!(page.mangas.len(), 1);
        assert_eq!(page.mangas[0].title, "Legend of the Northern Blade");
        assert_eq!(page.mangas[0].url, "/manga/legend-of-the-northern-blade/");
        assert_eq!(
            page.mangas[0].thumbnail_url.as_deref(),
            Some("https://leviatanscans.com/covers/legend-of-northern-blade.jpg")
        );

        let details_html = r##"
        <div class="post-title"><h1>Legend of the Northern Blade</h1></div>
        <div class="author-content"><a href="#">Hae-Min</a></div>
        <div class="artist-content"><a href="#">Hae-Min</a></div>
        <div class="genres-content"><a href="#">Action</a><a href="#">Murim</a></div>
        <div class="post-status"><div class="summary-content">OnGoing</div></div>
        <div class="description-summary"><div class="summary__content">Jin Mu-Won faces the Silent Night.</div></div>
        "##;

        let detailed = driver.parse_details_html(&page.mangas[0], details_html).unwrap();
        assert_eq!(detailed.author.as_deref(), Some("Hae-Min"));
        assert_eq!(detailed.genre, vec!["Action", "Murim"]);
        assert_eq!(detailed.status, MangaStatus::Ongoing);
        assert_eq!(detailed.description.as_deref(), Some("Jin Mu-Won faces the Silent Night."));

        let chapters_html = r#"
        <ul class="main version-chap">
            <li class="wp-manga-chapter">
                <a href="https://leviatanscans.com/manga/legend-of-the-northern-blade/chapter-202/">Chapter 202 - Finale</a>
            </li>
            <li class="wp-manga-chapter">
                <a href="https://leviatanscans.com/manga/legend-of-the-northern-blade/chapter-201-5/">Chapter 201.5</a>
            </li>
        </ul>
        "#;

        let chapters = driver.parse_chapters_html(&detailed.title, chapters_html).unwrap();
        assert_eq!(chapters.len(), 2);
        assert_eq!(chapters[0].chapter_number, 202.0);
        assert_eq!(chapters[0].url, "/manga/legend-of-the-northern-blade/chapter-202/");
        assert_eq!(chapters[1].chapter_number, 201.5);

        let pages_html = r#"
        <div class="reading-content">
            <div class="page-break"><img data-src="https://cdn.leviatanscans.com/ch202/01.webp" /></div>
            <div class="page-break"><img data-src="https://cdn.leviatanscans.com/ch202/02.webp" /></div>
        </div>
        "#;
        let pages = driver.parse_pages_html(pages_html).unwrap();
        assert_eq!(pages.len(), 2);
        assert_eq!(pages[0].index, 0);
        assert_eq!(pages[0].image_url.as_deref(), Some("https://cdn.leviatanscans.com/ch202/01.webp"));
        assert_eq!(pages[1].index, 1);
    }

    #[test]
    fn test_mangathemesia_ts_reader_and_chapter_parsing() {
        let driver = MangaThemesiaSource::new("Asura Scans", "https://asuracomic.net", "en", 1);

        let chapters_html = r#"
        <div id="chapterlist">
            <ul>
                <li data-num="145.5">
                    <div class="chbox">
                        <div class="eph-num">
                            <a href="https://asuracomic.net/solo-leveling-chapter-145-5/">
                                <span class="chapternum">Chapter 145.5</span>
                            </a>
                        </div>
                    </div>
                </li>
            </ul>
        </div>
        "#;
        let chapters = driver.parse_chapters_html("Solo Leveling", chapters_html).unwrap();
        assert_eq!(chapters.len(), 1);
        assert_eq!(chapters[0].chapter_number, 145.5);
        assert_eq!(chapters[0].url, "/solo-leveling-chapter-145-5/");

        let reader_html = r#"
        <html>
        <script>
        ts_reader.run({"sources":[{"source":"Default","images":["https://img.asura.net/01.jpg","https://img.asura.net/02.jpg"]}]});
        </script>
        </html>
        "#;
        let pages = driver.parse_pages_html(reader_html).unwrap();
        assert_eq!(pages.len(), 2);
        assert_eq!(pages[0].image_url.as_deref(), Some("https://img.asura.net/01.jpg"));
        assert_eq!(pages[1].image_url.as_deref(), Some("https://img.asura.net/02.jpg"));
    }

    #[test]
    fn test_mangadex_v5_json_parsing() {
        let driver = MangaDexSource::new("en");
        let manga_list_json = r#"{
            "result": "ok",
            "data": [
                {
                    "id": "a1b2c3d4",
                    "type": "manga",
                    "attributes": {
                        "title": { "en": "Chainsaw Man" },
                        "description": { "en": "Denji and Pochita." },
                        "status": "ongoing",
                        "tags": [
                            { "attributes": { "name": { "en": "Action" } } }
                        ]
                    },
                    "relationships": [
                        { "type": "author", "attributes": { "name": "Tatsuki Fujimoto" } },
                        { "type": "cover_art", "attributes": { "fileName": "cover123.jpg" } }
                    ]
                }
            ],
            "limit": 20,
            "offset": 0,
            "total": 45
        }"#;

        let page = driver.parse_manga_list_json(manga_list_json).unwrap();
        assert!(page.has_next_page);
        assert_eq!(page.mangas.len(), 1);
        assert_eq!(page.mangas[0].title, "Chainsaw Man");
        assert_eq!(page.mangas[0].author.as_deref(), Some("Tatsuki Fujimoto"));
        assert_eq!(
            page.mangas[0].thumbnail_url.as_deref(),
            Some("https://uploads.mangadex.org/covers/a1b2c3d4/cover123.jpg.256.jpg")
        );

        let at_home_json = r#"{
            "baseUrl": "https://uploads.mangadex.org",
            "chapter": {
                "hash": "deadbeef123",
                "data": ["p1.png", "p2.png"]
            }
        }"#;
        let pages = driver.parse_at_home_pages_json(at_home_json).unwrap();
        assert_eq!(pages.len(), 2);
        assert_eq!(
            pages[0].image_url.as_deref(),
            Some("https://uploads.mangadex.org/data/deadbeef123/p1.png")
        );
    }

    #[tokio::test]
    async fn test_local_source_and_global_search() {
        use ikazuchi_core::MangaSource;
        use std::sync::Arc;
        use std::time::Duration;

        let local_root = std::env::temp_dir().join("ikazuchi_local_source_test");
        let _ = std::fs::remove_dir_all(&local_root);
        let manga_dir = local_root.join("Dungeon Meshi");
        std::fs::create_dir_all(&manga_dir).unwrap();

        // Write a folder-level ComicInfo.xml for Dungeon Meshi
        let dir_comic_info = ikazuchi_archive::ComicInfoMetadata {
            series: Some("Delicious in Dungeon (Dungeon Meshi)".into()),
            writer: Some("Ryoko Kui".into()),
            summary: Some("Laios and his party cook monsters in the dungeon.".into()),
            genre: vec!["Fantasy".into(), "Gourmet".into()],
            publishing_status: Some(MangaStatus::Completed),
            ..Default::default()
        };
        std::fs::write(manga_dir.join("ComicInfo.xml"), dir_comic_info.to_xml()).unwrap();

        let png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 1, 2, 3];
        let ch_comic_info = ikazuchi_archive::ComicInfoMetadata {
            title: Some("Red Dragon Side Story".into()),
            number: Some(12.5),
            translator: Some("Everyday Heroes Scans".into()),
            ..Default::default()
        };
        ikazuchi_archive::write_cbz_archive_with_comic_info(
            manga_dir.join("Chapter 12.5 - Extra.cbz"),
            &[png.clone(), png],
            Some(&ch_comic_info),
        )
        .unwrap();

        let local_src = Arc::new(LocalSource::new(&local_root));
        assert_eq!(local_src.id(), LOCAL_SOURCE_ID);

        // Verify ComicInfo.xml metadata extraction for manga & chapter
        let popular = local_src.get_popular_manga(1).await.unwrap();
        assert_eq!(popular.mangas.len(), 1);
        assert_eq!(
            popular.mangas[0].title,
            "Delicious in Dungeon (Dungeon Meshi)"
        );
        assert_eq!(popular.mangas[0].author.as_deref(), Some("Ryoko Kui"));
        assert_eq!(popular.mangas[0].status, MangaStatus::Completed);

        let chs = local_src.get_chapter_list(&popular.mangas[0]).await.unwrap();
        assert_eq!(chs.len(), 1);
        assert_eq!(chs[0].name, "Red Dragon Side Story");
        assert!((chs[0].chapter_number - 12.5).abs() < 1e-3);
        assert_eq!(chs[0].scanlator.as_deref(), Some("Everyday Heroes Scans"));

        let pages = local_src.get_page_list(&chs[0]).await.unwrap();
        assert_eq!(pages.len(), 2);
        assert!(pages[0]
            .image_url
            .as_deref()
            .unwrap()
            .starts_with("ikazuchi://localhost/cbz/0?path="));

        // Verify path traversal safeguard
        let bad_chapter = ikazuchi_core::SChapter {
            url: "../etc/passwd".into(),
            name: "bad".into(),
            chapter_number: 1.0,
            date_upload: 0,
            scanlator: None,
        };
        assert!(local_src.get_page_list(&bad_chapter).await.is_err());

        // Verify Global Search via ExtensionManager
        let mut mgr = ExtensionManager::new();
        mgr.register_source(local_src);
        let hits = mgr.global_search("dungeon", Duration::from_secs(2)).await;
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].source_id, LOCAL_SOURCE_ID);
        assert_eq!(hits[0].mangas.len(), 1);

        let _ = std::fs::remove_dir_all(&local_root);
    }

    #[test]
    fn test_extension_settings_nsfw_language_pin_and_disable_filtering() {
        use ikazuchi_core::MangaSource;
        use std::sync::Arc;

        let index_json = r#"[
            {
                "name": "Tachiyomi: Asura Scans",
                "pkg": "eu.kanade.tachiyomi.extension.en.asurascans",
                "apk": "asura.apk",
                "lang": "en",
                "code": 45,
                "version": "1.4.45",
                "nsfw": 0,
                "sources": [{"name": "Asura Scans", "lang": "en", "id": "1001", "baseUrl": "https://asuracomic.net"}]
            },
            {
                "name": "Tachiyomi: AdultDoujin",
                "pkg": "eu.kanade.tachiyomi.extension.en.adultdoujin",
                "apk": "adult.apk",
                "lang": "en",
                "code": 12,
                "version": "1.4.12",
                "nsfw": 1,
                "sources": [{"name": "AdultDoujin", "lang": "en", "id": "9999", "baseUrl": "https://adult.example"}]
            },
            {
                "name": "Tachiyomi: RawSenManga",
                "pkg": "eu.kanade.tachiyomi.extension.ja.rawsenmanga",
                "apk": "rawsen.apk",
                "lang": "ja",
                "code": 10,
                "version": "1.4.10",
                "nsfw": 0,
                "sources": [{"name": "RawSenManga", "lang": "ja", "id": "2002", "baseUrl": "https://raw.senmanga.com"}]
            }
        ]"#;

        let entries = parse_mihon_repo_index_json(index_json).unwrap();
        assert_eq!(entries.len(), 3);

        // 1. Default settings (`show_nsfw = false`, `enabled_languages = ["en", "all"]`):
        // Only "Asura Scans" is visible (AdultDoujin is NSFW, RawSenManga is "ja").
        let mut settings = ExtensionSettings::default();
        let default_filtered = filter_repo_extensions(&entries, &settings);
        assert_eq!(default_filtered.len(), 1);
        assert_eq!(default_filtered[0].clean_name(), "Asura Scans");

        // 2. Enable NSFW (`show_nsfw = true`) -> AdultDoujin appears alongside Asura Scans
        settings.show_nsfw = true;
        let nsfw_enabled = filter_repo_extensions(&entries, &settings);
        assert_eq!(nsfw_enabled.len(), 2);

        // 3. Also enable Japanese (`"ja"`) -> all 3 extensions appear
        settings.enabled_languages.push("ja".into());
        let all_three = filter_repo_extensions(&entries, &settings);
        assert_eq!(all_three.len(), 3);

        // 4. Test installed sources filtering (`ExtensionManager::visible_sources`)
        let mut mgr = ExtensionManager::new();
        let asura: Arc<dyn MangaSource> =
            Arc::new(MangaThemesiaSource::new("Asura Scans", "https://asuracomic.net", "en", 1));
        let leviatan: Arc<dyn MangaSource> =
            Arc::new(MadaraSource::new("Leviatan Scans", "https://leviatanscans.com", "en", 1));
        let nsfw_src: Arc<dyn MangaSource> =
            Arc::new(MadaraSource::new("NsfwScans", "https://nsfw.example", "en", 1));
        let ja_src: Arc<dyn MangaSource> =
            Arc::new(MadaraSource::new("RawSen", "https://raw.example", "ja", 1));

        let leviatan_id = leviatan.id();
        let asura_id = asura.id();

        mgr.register_source_with_nsfw(asura, false);
        mgr.register_source_with_nsfw(leviatan, false);
        mgr.register_source_with_nsfw(nsfw_src, true);
        mgr.register_source_with_nsfw(ja_src, false);

        // With default settings (`show_nsfw = false`, `enabled_languages = ["en", "all"]`)
        // and Leviatan pinned: Leviatan comes first, Asura second, NsfwScans & RawSen hidden!
        let mut ext_cfg = ExtensionSettings::default();
        ext_cfg.pinned_source_ids.push(leviatan_id);

        let visible = mgr.visible_sources(&ext_cfg);
        assert_eq!(visible.len(), 2);
        assert_eq!(visible[0].name(), "Leviatan Scans");
        assert_eq!(visible[1].name(), "Asura Scans");

        // Disable Asura Scans -> only Leviatan remains
        ext_cfg.disabled_source_ids.push(asura_id);
        let visible_after_disable = mgr.visible_sources(&ext_cfg);
        assert_eq!(visible_after_disable.len(), 1);
        assert_eq!(visible_after_disable[0].name(), "Leviatan Scans");
    }
}
