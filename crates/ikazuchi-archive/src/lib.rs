//! # `ikazuchi-archive`
//!
//! Zero-copy `.cbz` / `.zip` archive reader, atomic validated `.cbz` writer,
//! Anansi `ComicInfo.xml` v2.0 metadata parser/generator (Yokai/Mihon compatible),
//! cross-platform path sanitizer, and chapter download queue manager.

use ikazuchi_core::{IkazuchiError, MangaStatus, Result};
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use zip::write::SimpleFileOptions;
use zip::{ZipArchive, ZipWriter};

/// Anansi `ComicInfo.xml` v2.0 metadata embedded inside `.cbz` archives or manga folders
/// (compatible with Yokai's `yokai.core.metadata.ComicInfo` and Mihon).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ComicInfoMetadata {
    /// `<Title>` — Chapter title
    pub title: Option<String>,
    /// `<Series>` — Manga title
    pub series: Option<String>,
    /// `<Number>` — Chapter number
    pub number: Option<f32>,
    /// `<Summary>` — Manga description/synopsis
    pub summary: Option<String>,
    /// `<Writer>` — Manga author
    pub writer: Option<String>,
    /// `<Penciller>` — Manga artist
    pub penciller: Option<String>,
    /// `<Translator>` — Scanlator group
    pub translator: Option<String>,
    /// `<Genre>` — Comma-separated genres
    pub genre: Vec<String>,
    /// `<ty:PublishingStatusTachiyomi>` — Ongoing, Completed, etc.
    pub publishing_status: Option<MangaStatus>,
    /// `<mh:SourceMihon>` — Source name
    pub source_name: Option<String>,
    /// `<j2k:LanguageJ2K>` — ISO language code
    pub language: Option<String>,
}

impl ComicInfoMetadata {
    /// Serializes this metadata to a valid XML 1.0 `ComicInfo.xml` document.
    pub fn to_xml(&self) -> String {
        let mut xml = String::from(
            r#"<?xml version="1.0" encoding="utf-8"?>
<ComicInfo xmlns:xsd="http://www.w3.org/2001/XMLSchema" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xmlns:ty="http://www.w3.org/2001/XMLSchema" xmlns:mh="http://www.w3.org/2001/XMLSchema" xmlns:j2k="http://www.w3.org/2001/XMLSchema">
"#,
        );
        if let Some(t) = &self.title {
            xml.push_str(&format!("  <Title>{}</Title>\n", escape_xml(t)));
        }
        if let Some(s) = &self.series {
            xml.push_str(&format!("  <Series>{}</Series>\n", escape_xml(s)));
        }
        if let Some(n) = self.number.filter(|&n| n >= 0.0) {
            let num_str = if (n.fract()).abs() < 1e-4 {
                format!("{}", n as i64)
            } else {
                format!("{n}")
            };
            xml.push_str(&format!("  <Number>{num_str}</Number>\n"));
        }
        if let Some(sum) = &self.summary {
            xml.push_str(&format!("  <Summary>{}</Summary>\n", escape_xml(sum)));
        }
        if let Some(w) = &self.writer {
            xml.push_str(&format!("  <Writer>{}</Writer>\n", escape_xml(w)));
        }
        if let Some(p) = &self.penciller {
            xml.push_str(&format!("  <Penciller>{}</Penciller>\n", escape_xml(p)));
        }
        if let Some(tr) = &self.translator {
            xml.push_str(&format!("  <Translator>{}</Translator>\n", escape_xml(tr)));
        }
        if !self.genre.is_empty() {
            let joined = self.genre.join(", ");
            xml.push_str(&format!("  <Genre>{}</Genre>\n", escape_xml(&joined)));
        }
        if let Some(st) = self.publishing_status {
            let label = match st {
                MangaStatus::Ongoing => "Ongoing",
                MangaStatus::Completed => "Completed",
                MangaStatus::Licensed => "Licensed",
                MangaStatus::PublishingFinished => "Publishing finished",
                MangaStatus::Cancelled => "Cancelled",
                MangaStatus::OnHiatus => "On hiatus",
                MangaStatus::Unknown => "Unknown",
            };
            xml.push_str(&format!(
                "  <ty:PublishingStatusTachiyomi>{label}</ty:PublishingStatusTachiyomi>\n"
            ));
        }
        if let Some(src) = &self.source_name {
            xml.push_str(&format!(
                "  <mh:SourceMihon>{}</mh:SourceMihon>\n",
                escape_xml(src)
            ));
        }
        if let Some(lang) = &self.language {
            xml.push_str(&format!(
                "  <j2k:LanguageJ2K>{}</j2k:LanguageJ2K>\n",
                escape_xml(lang)
            ));
        }
        xml.push_str("</ComicInfo>\n");
        xml
    }

    /// Parses a `ComicInfo.xml` document (matching Yokai's `ComicInfo` tags with or without XML namespace prefixes).
    pub fn from_xml(xml: &str) -> Self {
        let genre_raw = extract_xml_tag(xml, "Genre");
        let tags_raw = extract_xml_tag(xml, "Tags");
        let mut genre = Vec::new();
        for raw in [genre_raw, tags_raw].into_iter().flatten() {
            for part in raw.split(',') {
                let trimmed = part.trim().to_string();
                if !trimmed.is_empty() && !genre.contains(&trimmed) {
                    genre.push(trimmed);
                }
            }
        }

        let number = extract_xml_tag(xml, "Number").and_then(|s| s.trim().parse::<f32>().ok());
        let publishing_status = extract_xml_tag(xml, "PublishingStatusTachiyomi").map(|s| {
            match s.trim().to_ascii_lowercase().as_str() {
                "ongoing" => MangaStatus::Ongoing,
                "completed" => MangaStatus::Completed,
                "licensed" => MangaStatus::Licensed,
                "publishing finished" => MangaStatus::PublishingFinished,
                "cancelled" => MangaStatus::Cancelled,
                "on hiatus" => MangaStatus::OnHiatus,
                _ => MangaStatus::Unknown,
            }
        });

        Self {
            title: extract_xml_tag(xml, "Title"),
            series: extract_xml_tag(xml, "Series"),
            number,
            summary: extract_xml_tag(xml, "Summary"),
            writer: extract_xml_tag(xml, "Writer"),
            penciller: extract_xml_tag(xml, "Penciller"),
            translator: extract_xml_tag(xml, "Translator"),
            genre,
            publishing_status,
            source_name: extract_xml_tag(xml, "SourceMihon"),
            language: extract_xml_tag(xml, "LanguageJ2K")
                .or_else(|| extract_xml_tag(xml, "LanguageISO")),
        }
    }
}

fn escape_xml(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for c in input.chars() {
        let code = c as u32;
        let valid_xml10 = code == 0x9
            || code == 0xA
            || code == 0xD
            || (0x20..=0xD7FF).contains(&code)
            || (0xE000..=0xFFFD).contains(&code)
            || (0x10000..=0x10FFFF).contains(&code);
        if !valid_xml10 {
            continue;
        }
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

fn unescape_xml(input: &str) -> String {
    input
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

fn extract_xml_tag(xml: &str, local_name: &str) -> Option<String> {
    for open_pat in [
        format!("<{local_name}>"),
        format!("<ty:{local_name}>"),
        format!("<mh:{local_name}>"),
        format!("<j2k:{local_name}>"),
    ] {
        if let Some(start_idx) = xml.find(&open_pat) {
            let content_start = start_idx + open_pat.len();
            let close_pat = open_pat.replacen('<', "</", 1);
            if let Some(rel_end) = xml[content_start..].find(&close_pat) {
                let raw = xml[content_start..content_start + rel_end].trim();
                if !raw.is_empty() {
                    return Some(unescape_xml(raw));
                }
            }
        }
    }
    None
}

/// Natural alphanumeric comparator so `"2.jpg"` sorts before `"10.jpg"`
/// even when an archive creator did not zero-pad page filenames.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let mut chars_a = a.chars().peekable();
    let mut chars_b = b.chars().peekable();

    while let (Some(&ca), Some(&cb)) = (chars_a.peek(), chars_b.peek()) {
        if ca.is_ascii_digit() && cb.is_ascii_digit() {
            let mut num_a = String::new();
            while let Some(&c) = chars_a.peek() {
                if c.is_ascii_digit() {
                    num_a.push(c);
                    chars_a.next();
                } else {
                    break;
                }
            }
            let mut num_b = String::new();
            while let Some(&c) = chars_b.peek() {
                if c.is_ascii_digit() {
                    num_b.push(c);
                    chars_b.next();
                } else {
                    break;
                }
            }
            let trimmed_a = num_a.trim_start_matches('0');
            let trimmed_b = num_b.trim_start_matches('0');
            match trimmed_a.len().cmp(&trimmed_b.len()) {
                Ordering::Equal => match trimmed_a.cmp(trimmed_b) {
                    Ordering::Equal => match num_a.len().cmp(&num_b.len()) {
                        Ordering::Equal => continue,
                        other => return other,
                    },
                    other => return other,
                },
                other => return other,
            }
        } else {
            let la = ca.to_ascii_lowercase();
            let lb = cb.to_ascii_lowercase();
            match la.cmp(&lb) {
                Ordering::Equal => {
                    chars_a.next();
                    chars_b.next();
                }
                other => return other,
            }
        }
    }
    chars_a.count().cmp(&chars_b.count())
}

/// Sanitizes a source, manga, or chapter name into a safe cross-platform filesystem component
/// across Linux, Android, and Windows.
///
/// Safeguards against:
/// - Illegal characters (`/`, `\`, `:`, `*`, `?`, `"`, `<`, `>`, `|`, control chars)
/// - Path traversal (`..`, `.`)
/// - Trailing dots or spaces (which corrupt filenames on Windows/FAT32)
/// - Windows reserved device names (`CON`, `PRN`, `AUX`, `NUL`, `COM1..9`, `LPT1..9`)
pub fn sanitize_fs_component(raw: &str) -> String {
    let mut cleaned: String = raw
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0'..='\x1F' => '_',
            _ => c,
        })
        .collect();

    cleaned = cleaned.trim().trim_end_matches(['.', ' ']).to_string();

    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        return "_unnamed".to_string();
    }

    let upper = cleaned.to_ascii_uppercase();
    let base_upper = upper.split('.').next().unwrap_or("");
    let is_reserved = matches!(
        base_upper,
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    );

    if is_reserved {
        cleaned = format!("_{cleaned}");
    }

    // Cap component length to 120 chars to stay well below 255-byte fs limits
    if cleaned.chars().count() > 120 {
        cleaned = cleaned.chars().take(120).collect();
        cleaned = cleaned.trim_end_matches(['.', ' ']).to_string();
    }

    cleaned
}

/// Inspects raw image magic bytes and returns the canonical file extension (`"png"`, `"jpg"`, `"webp"`, `"gif"`, `"avif"`),
/// or `None` if the buffer is empty or is not a valid image (e.g. a Cloudflare HTML 403 error page).
pub fn detect_image_extension(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        Some("png")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("jpg")
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("webp")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("gif")
    } else if bytes.len() >= 12 && &bytes[4..8] == b"ftyp" && (&bytes[8..12] == b"avif" || &bytes[8..12] == b"avis") {
        Some("avif")
    } else {
        None
    }
}

/// Returns the canonical HTTP MIME type for raw image bytes, or `"application/octet-stream"`.
pub fn detect_image_mime(bytes: &[u8]) -> &'static str {
    match detect_image_extension(bytes) {
        Some("png") => "image/png",
        Some("jpg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("gif") => "image/gif",
        Some("avif") => "image/avif",
        _ => "application/octet-stream",
    }
}

/// Lists naturally-sorted image entry names inside a `.cbz` or `.zip` chapter archive.
pub fn list_cbz_pages(cbz_path: impl AsRef<Path>) -> Result<Vec<String>> {
    let file = File::open(cbz_path).map_err(|e| IkazuchiError::Parse(e.to_string()))?;
    let mut archive = ZipArchive::new(file).map_err(|e| IkazuchiError::Parse(e.to_string()))?;

    let mut names: Vec<String> = (0..archive.len())
        .filter_map(|i| {
            let entry = archive.by_index(i).ok()?;
            if entry.is_dir() {
                return None;
            }
            let name = entry.name().to_string();
            let lower = name.to_lowercase();
            if lower.ends_with(".jpg")
                || lower.ends_with(".jpeg")
                || lower.ends_with(".png")
                || lower.ends_with(".webp")
                || lower.ends_with(".gif")
                || lower.ends_with(".avif")
            {
                Some(name)
            } else {
                None
            }
        })
        .collect();

    names.sort_by(|a, b| natural_cmp(a, b));
    Ok(names)
}

/// Reads a single page's raw image bytes from a `.cbz` archive by its 0-based page index.
pub fn read_cbz_page_bytes(cbz_path: impl AsRef<Path>, page_index: usize) -> Result<Vec<u8>> {
    let pages = list_cbz_pages(&cbz_path)?;
    let entry_name = pages
        .get(page_index)
        .ok_or_else(|| IkazuchiError::Parse(format!("Page index {page_index} out of bounds")))?;

    let file = File::open(cbz_path).map_err(|e| IkazuchiError::Parse(e.to_string()))?;
    let mut archive = ZipArchive::new(file).map_err(|e| IkazuchiError::Parse(e.to_string()))?;
    let mut entry = archive
        .by_name(entry_name)
        .map_err(|e| IkazuchiError::Parse(e.to_string()))?;

    let mut buf = Vec::with_capacity(entry.size() as usize);
    entry
        .read_to_end(&mut buf)
        .map_err(|e| IkazuchiError::Parse(e.to_string()))?;
    Ok(buf)
}

/// Reads and parses `ComicInfo.xml` from inside a `.cbz` or `.zip` chapter archive if present.
pub fn read_cbz_comic_info(cbz_path: impl AsRef<Path>) -> Option<ComicInfoMetadata> {
    let file = File::open(cbz_path).ok()?;
    let mut archive = ZipArchive::new(file).ok()?;

    let mut xml_index = None;
    for i in 0..archive.len() {
        if let Ok(entry) = archive.by_index(i) {
            if !entry.is_dir() && entry.name().eq_ignore_ascii_case("ComicInfo.xml") {
                xml_index = Some(i);
                break;
            }
        }
    }

    let mut entry = archive.by_index(xml_index?).ok()?;
    let mut xml_str = String::new();
    entry.read_to_string(&mut xml_str).ok()?;
    Some(ComicInfoMetadata::from_xml(&xml_str))
}

/// Writes chapter images into a standard `.cbz` archive.
pub fn write_cbz_archive(cbz_path: impl AsRef<Path>, pages: &[(String, Vec<u8>)]) -> Result<()> {
    let path = cbz_path.as_ref();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| IkazuchiError::Parse(e.to_string()))?;
    }
    let file = File::create(path).map_err(|e| IkazuchiError::Parse(e.to_string()))?;
    let mut zip = ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);

    for (filename, bytes) in pages {
        zip.start_file(filename, options)
            .map_err(|e| IkazuchiError::Parse(e.to_string()))?;
        zip.write_all(bytes)
            .map_err(|e| IkazuchiError::Parse(e.to_string()))?;
    }

    zip.finish().map_err(|e| IkazuchiError::Parse(e.to_string()))?;
    Ok(())
}

/// Atomically writes a validated chapter `.cbz` archive:
/// 1. Rejects empty page lists.
/// 2. Verifies every page's magic bytes (`PNG`, `JPEG`, `WEBP`, `GIF`, `AVIF`) so HTML error pages
///    or truncated 0-byte buffers never corrupt a downloaded chapter.
/// 3. Optionally embeds `ComicInfo.xml` inside the `.cbz` archive (Yokai/Mihon compatibility).
/// 4. Writes to `<cbz_path>.part` first, flushes, and atomically renames `.part` -> `.cbz`.
pub fn write_cbz_archive_validated_atomic(
    cbz_path: impl AsRef<Path>,
    raw_pages: &[Vec<u8>],
) -> Result<PathBuf> {
    write_cbz_archive_with_comic_info(cbz_path, raw_pages, None)
}

/// Atomically writes a validated chapter `.cbz` archive and embeds `ComicInfo.xml` when `comic_info` is `Some`.
pub fn write_cbz_archive_with_comic_info(
    cbz_path: impl AsRef<Path>,
    raw_pages: &[Vec<u8>],
    comic_info: Option<&ComicInfoMetadata>,
) -> Result<PathBuf> {
    if raw_pages.is_empty() {
        return Err(IkazuchiError::Parse(
            "Cannot write .cbz archive with 0 pages".into(),
        ));
    }

    let mut named_pages = Vec::with_capacity(raw_pages.len() + 1);
    if let Some(info) = comic_info {
        named_pages.push(("ComicInfo.xml".to_string(), info.to_xml().into_bytes()));
    }

    for (idx, bytes) in raw_pages.iter().enumerate() {
        let ext = detect_image_extension(bytes).ok_or_else(|| {
            IkazuchiError::Parse(format!(
                "Page {} failed image magic-byte validation (received {} bytes, possibly HTML/Cloudflare challenge)",
                idx + 1,
                bytes.len()
            ))
        })?;
        named_pages.push((format!("{:03}.{ext}", idx + 1), bytes.clone()));
    }

    let final_path = cbz_path.as_ref().to_path_buf();
    if let Some(parent) = final_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| IkazuchiError::Parse(e.to_string()))?;
    }

    let part_path = final_path.with_extension("cbz.part");
    if let Err(e) = write_cbz_archive(&part_path, &named_pages) {
        let _ = std::fs::remove_file(&part_path);
        return Err(e);
    }

    std::fs::rename(&part_path, &final_path).map_err(|e| {
        let _ = std::fs::remove_file(&part_path);
        IkazuchiError::Parse(format!("Failed to rename .cbz.part to final .cbz: {e}"))
    })?;

    Ok(final_path)
}

/// Download status of a chapter in the [`ChapterDownloadManager`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChapterDownloadState {
    NotDownloaded,
    Queued,
    Downloading {
        downloaded_pages: usize,
        total_pages: usize,
    },
    Downloaded {
        cbz_path: PathBuf,
        page_count: usize,
    },
    Failed {
        reason: String,
    },
}

/// Manages offline `.cbz` downloads, directory layout, integrity checks, and delete-after-reading.
#[derive(Debug, Clone)]
pub struct ChapterDownloadManager {
    pub root_dir: PathBuf,
    pub delete_after_reading: bool,
    states: BTreeMap<i64, ChapterDownloadState>,
}

impl ChapterDownloadManager {
    pub fn new(root_dir: impl Into<PathBuf>, delete_after_reading: bool) -> Self {
        Self {
            root_dir: root_dir.into(),
            delete_after_reading,
            states: BTreeMap::new(),
        }
    }

    /// Computes the canonical, sanitized `.cbz` file path for a `(source_name, manga_title, chapter_name)`.
    pub fn chapter_cbz_path(
        &self,
        source_name: &str,
        manga_title: &str,
        chapter_name: &str,
    ) -> PathBuf {
        let src_dir = sanitize_fs_component(source_name);
        let manga_dir = sanitize_fs_component(manga_title);
        let ch_file = format!("{}.cbz", sanitize_fs_component(chapter_name));
        self.root_dir.join(src_dir).join(manga_dir).join(ch_file)
    }

    /// Enqueues a chapter ID for download.
    pub fn enqueue(&mut self, chapter_id: i64) {
        self.states.insert(chapter_id, ChapterDownloadState::Queued);
    }

    /// Updates progress for an active chapter download.
    pub fn report_progress(&mut self, chapter_id: i64, downloaded_pages: usize, total_pages: usize) {
        self.states.insert(
            chapter_id,
            ChapterDownloadState::Downloading {
                downloaded_pages,
                total_pages,
            },
        );
    }

    /// Saves a chapter's raw page buffers atomically to disk, validating every image header
    /// and embedding a standard `ComicInfo.xml` manifest inside the `.cbz` archive.
    pub fn complete_chapter_download(
        &mut self,
        chapter_id: i64,
        source_name: &str,
        manga_title: &str,
        chapter_name: &str,
        raw_pages: &[Vec<u8>],
    ) -> Result<PathBuf> {
        let target_path = self.chapter_cbz_path(source_name, manga_title, chapter_name);
        let comic_info = ComicInfoMetadata {
            title: Some(chapter_name.to_string()),
            series: Some(manga_title.to_string()),
            source_name: Some(source_name.to_string()),
            ..Default::default()
        };
        match write_cbz_archive_with_comic_info(&target_path, raw_pages, Some(&comic_info)) {
            Ok(path) => {
                self.states.insert(
                    chapter_id,
                    ChapterDownloadState::Downloaded {
                        cbz_path: path.clone(),
                        page_count: raw_pages.len(),
                    },
                );
                Ok(path)
            }
            Err(err) => {
                self.states.insert(
                    chapter_id,
                    ChapterDownloadState::Failed {
                        reason: err.to_string(),
                    },
                );
                Err(err)
            }
        }
    }

    /// Checks the download state of a chapter, verifying on-disk `.cbz` integrity if present.
    pub fn get_state(
        &self,
        chapter_id: i64,
        source_name: &str,
        manga_title: &str,
        chapter_name: &str,
    ) -> ChapterDownloadState {
        if let Some(state) = self.states.get(&chapter_id) {
            if let ChapterDownloadState::Downloaded { cbz_path, page_count } = state {
                if cbz_path.exists() {
                    return ChapterDownloadState::Downloaded {
                        cbz_path: cbz_path.clone(),
                        page_count: *page_count,
                    };
                }
            } else {
                return state.clone();
            }
        }

        let path = self.chapter_cbz_path(source_name, manga_title, chapter_name);
        if path.exists() {
            if let Ok(pages) = list_cbz_pages(&path) {
                if !pages.is_empty() {
                    return ChapterDownloadState::Downloaded {
                        cbz_path: path,
                        page_count: pages.len(),
                    };
                }
            }
        }
        ChapterDownloadState::NotDownloaded
    }

    /// If `delete_after_reading` is enabled (or `force` is true), removes the downloaded `.cbz` archive
    /// and resets the chapter state to `NotDownloaded`.
    pub fn on_chapter_marked_read(
        &mut self,
        chapter_id: i64,
        source_name: &str,
        manga_title: &str,
        chapter_name: &str,
        force_delete: bool,
    ) -> bool {
        if !self.delete_after_reading && !force_delete {
            return false;
        }
        let path = self.chapter_cbz_path(source_name, manga_title, chapter_name);
        self.states.remove(&chapter_id);
        if path.exists() {
            std::fs::remove_file(path).is_ok()
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cbz_write_and_stream_read_with_natural_sorting() {
        let tmp_path = std::env::temp_dir().join("ikazuchi_test_chapter.cbz");
        let pages = vec![
            ("10.webp".to_string(), vec![9, 9, 9]),
            ("2.webp".to_string(), vec![2, 2, 2]),
            ("1.jpg".to_string(), vec![1, 1, 1]),
        ];

        write_cbz_archive(&tmp_path, &pages).unwrap();
        let listed = list_cbz_pages(&tmp_path).unwrap();
        // Verify natural sort puts "1.jpg", "2.webp", "10.webp" in numerical order!
        assert_eq!(listed, vec!["1.jpg", "2.webp", "10.webp"]);

        let page0 = read_cbz_page_bytes(&tmp_path, 0).unwrap();
        assert_eq!(page0, vec![1, 1, 1]);

        let page1 = read_cbz_page_bytes(&tmp_path, 1).unwrap();
        assert_eq!(page1, vec![2, 2, 2]);

        let page2 = read_cbz_page_bytes(&tmp_path, 2).unwrap();
        assert_eq!(page2, vec![9, 9, 9]);

        let _ = std::fs::remove_file(&tmp_path);
    }

    #[test]
    fn test_sanitize_fs_component_and_atomic_cbz_downloader_safeguards() {
        assert_eq!(
            sanitize_fs_component("Re:Zero - Starting Life in Another World? .. "),
            "Re_Zero - Starting Life in Another World_"
        );
        assert_eq!(sanitize_fs_component("CON"), "_CON");
        assert_eq!(sanitize_fs_component(".."), "_unnamed");

        let dl_root = std::env::temp_dir().join("ikazuchi_dl_mgr_test");
        let _ = std::fs::remove_dir_all(&dl_root);
        let mut mgr = ChapterDownloadManager::new(&dl_root, true);

        // 1. Attempt to save a Cloudflare HTML error page masquerading as an image -> MUST BE REJECTED!
        let fake_html = b"<html><body>403 Forbidden Cloudflare</body></html>".to_vec();
        let err = mgr.complete_chapter_download(
            101,
            "Asura Scans",
            "Re:Zero?",
            "Ch. 1: The End",
            &[fake_html],
        );
        assert!(err.is_err());

        // 2. Save valid PNG & JPEG pages -> succeeds atomically!
        let valid_png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 1, 2, 3];
        let valid_jpg = vec![0xFF, 0xD8, 0xFF, 0xE0, 4, 5, 6];
        let saved_path = mgr
            .complete_chapter_download(
                101,
                "Asura Scans",
                "Re:Zero?",
                "Ch. 1: The End",
                &[valid_png, valid_jpg],
            )
            .unwrap();
        assert!(saved_path.exists());

        // Verify ComicInfo.xml was embedded inside the downloaded .cbz archive!
        let embedded_info = read_cbz_comic_info(&saved_path).expect("ComicInfo.xml should be present");
        assert_eq!(embedded_info.title.as_deref(), Some("Ch. 1: The End"));
        assert_eq!(embedded_info.series.as_deref(), Some("Re:Zero?"));
        assert_eq!(embedded_info.source_name.as_deref(), Some("Asura Scans"));

        assert_eq!(
            mgr.get_state(101, "Asura Scans", "Re:Zero?", "Ch. 1: The End"),
            ChapterDownloadState::Downloaded {
                cbz_path: saved_path.clone(),
                page_count: 2
            }
        );

        // 3. Marking chapter read triggers automatic delete-after-reading cleanup
        assert!(mgr.on_chapter_marked_read(101, "Asura Scans", "Re:Zero?", "Ch. 1: The End", false));
        assert!(!saved_path.exists());
        let _ = std::fs::remove_dir_all(&dl_root);
    }
}
