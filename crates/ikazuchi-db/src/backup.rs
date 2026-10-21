//! # Mihon & Fork `.tachibk` / `.proto.gz` Backup Importer & Exporter
//!
//! Implements wire-compatible Protocol Buffer decoding/encoding and Gzip compression
//! for Mihon (`eu.kanade.tachiyomi.data.backup.models.*`) and its forks (Tachiyomi,
//! TachiyomiSY, Komikku, Yokai, J2K).

use crate::{CustomMangaInfo, Database, MangaTrackRecord};
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use ikazuchi_core::{
    IkazuchiError, MangaStatus, Result, SChapter, SManga, UpdateStrategy, url_without_domain,
};
use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Write};

/// Root Mihon backup payload (`Backup.kt`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MihonBackup {
    /// `@ProtoNumber(1)`
    pub backup_manga: Vec<BackupManga>,
    /// `@ProtoNumber(2)`
    pub backup_categories: Vec<BackupCategory>,
    /// `@ProtoNumber(101)`
    pub backup_sources: Vec<BackupSource>,
    /// `@ProtoNumber(106)`
    pub backup_extension_stores: Vec<BackupExtensionStore>,
}

/// `BackupManga.kt`
#[derive(Debug, Clone, PartialEq)]
pub struct BackupManga {
    /// `@ProtoNumber(1)`
    pub source: i64,
    /// `@ProtoNumber(2)`
    pub url: String,
    /// `@ProtoNumber(3)`
    pub title: String,
    /// `@ProtoNumber(4)`
    pub artist: Option<String>,
    /// `@ProtoNumber(5)`
    pub author: Option<String>,
    /// `@ProtoNumber(6)`
    pub description: Option<String>,
    /// `@ProtoNumber(7)`
    pub genre: Vec<String>,
    /// `@ProtoNumber(8)`
    pub status: i32,
    /// `@ProtoNumber(9)`
    pub thumbnail_url: Option<String>,
    /// `@ProtoNumber(13)` (epoch milliseconds)
    pub date_added: i64,
    /// `@ProtoNumber(14)` (legacy viewer value)
    pub viewer: i32,
    /// `@ProtoNumber(16)`
    pub chapters: Vec<BackupChapter>,
    /// `@ProtoNumber(17)` (references `BackupCategory.order` or 0-based index)
    pub categories: Vec<i64>,
    /// `@ProtoNumber(18)`
    pub tracking: Vec<BackupTracking>,
    /// `@ProtoNumber(100)` — NOTE: defaults to `true` in Mihon when omitted on wire!
    pub favorite: bool,
    /// `@ProtoNumber(101)`
    pub chapter_flags: i32,
    /// `@ProtoNumber(103)`
    pub viewer_flags: Option<i32>,
    /// `@ProtoNumber(104)`
    pub history: Vec<BackupHistory>,
    /// `@ProtoNumber(105)`
    pub update_strategy: i32,
    /// `@ProtoNumber(107)` (epoch seconds in legacy trigger)
    pub favorite_modified_at: Option<i64>,
    /// `@ProtoNumber(108)`
    pub excluded_scanlators: Vec<String>,
    /// `@ProtoNumber(110)`
    pub notes: String,
    /// `@ProtoNumber(111)`
    pub initialized: bool,
    /// `@ProtoNumber(602)` — TachiyomiSY / Yokai custom status override (`0` = None)
    pub custom_status: i32,
    /// `@ProtoNumber(800)` — J2K / Yokai custom title override
    pub custom_title: Option<String>,
    /// `@ProtoNumber(801)` — J2K / Yokai custom artist override
    pub custom_artist: Option<String>,
    /// `@ProtoNumber(802)` — J2K / Yokai custom author override
    pub custom_author: Option<String>,
    /// `@ProtoNumber(804)` — J2K / Yokai custom description override
    pub custom_description: Option<String>,
    /// `@ProtoNumber(805)` — J2K / Yokai custom genre overrides
    pub custom_genre: Vec<String>,
}

impl Default for BackupManga {
    fn default() -> Self {
        Self {
            source: 0,
            url: String::new(),
            title: String::new(),
            artist: None,
            author: None,
            description: None,
            genre: Vec::new(),
            status: 0,
            thumbnail_url: None,
            date_added: 0,
            viewer: 0,
            chapters: Vec::new(),
            categories: Vec::new(),
            tracking: Vec::new(),
            // CRITICAL SAFEGUARD: Mihon's BackupManga defines `@ProtoNumber(100) var favorite: Boolean = true`.
            // When kotlinx.serialization.protobuf omits default fields, `favorite` is true!
            favorite: true,
            chapter_flags: 0,
            viewer_flags: None,
            history: Vec::new(),
            update_strategy: 0,
            favorite_modified_at: None,
            excluded_scanlators: Vec::new(),
            notes: String::new(),
            initialized: false,
            custom_status: 0,
            custom_title: None,
            custom_artist: None,
            custom_author: None,
            custom_description: None,
            custom_genre: Vec::new(),
        }
    }
}

/// `BackupChapter.kt`
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BackupChapter {
    /// `@ProtoNumber(1)`
    pub url: String,
    /// `@ProtoNumber(2)`
    pub name: String,
    /// `@ProtoNumber(3)`
    pub scanlator: Option<String>,
    /// `@ProtoNumber(4)`
    pub read: bool,
    /// `@ProtoNumber(5)`
    pub bookmark: bool,
    /// `@ProtoNumber(6)`
    pub last_page_read: i64,
    /// `@ProtoNumber(7)`
    pub date_fetch: i64,
    /// `@ProtoNumber(8)`
    pub date_upload: i64,
    /// `@ProtoNumber(9)`
    pub chapter_number: f32,
    /// `@ProtoNumber(10)`
    pub source_order: i64,
}

/// `BackupCategory.kt`
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BackupCategory {
    /// `@ProtoNumber(1)`
    pub name: String,
    /// `@ProtoNumber(2)`
    pub order: i64,
    /// `@ProtoNumber(3)`
    pub id: i64,
    /// `@ProtoNumber(100)`
    pub flags: i64,
}

/// `BackupHistory.kt`
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BackupHistory {
    /// `@ProtoNumber(1)`
    pub url: String,
    /// `@ProtoNumber(2)`
    pub last_read: i64,
    /// `@ProtoNumber(3)`
    pub read_duration: i64,
}

/// `BackupTracking.kt`
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BackupTracking {
    /// `@ProtoNumber(1)`
    pub sync_id: i32,
    /// `@ProtoNumber(2)`
    pub library_id: i64,
    /// `@ProtoNumber(3)` (deprecated int mediaId)
    pub media_id_int: i32,
    /// `@ProtoNumber(4)`
    pub tracking_url: String,
    /// `@ProtoNumber(5)`
    pub title: String,
    /// `@ProtoNumber(6)`
    pub last_chapter_read: f32,
    /// `@ProtoNumber(7)`
    pub total_chapters: i32,
    /// `@ProtoNumber(8)`
    pub score: f32,
    /// `@ProtoNumber(9)`
    pub status: i32,
    /// `@ProtoNumber(10)`
    pub started_reading_date: i64,
    /// `@ProtoNumber(11)`
    pub finished_reading_date: i64,
    /// `@ProtoNumber(12)`
    pub private: bool,
    /// `@ProtoNumber(100)`
    pub media_id: i64,
}

impl BackupTracking {
    pub fn resolved_remote_id(&self) -> i64 {
        if self.media_id_int != 0 {
            self.media_id_int as i64
        } else {
            self.media_id
        }
    }
}

/// `BackupSource.kt`
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BackupSource {
    /// `@ProtoNumber(1)`
    pub name: String,
    /// `@ProtoNumber(2)`
    pub source_id: i64,
}

/// `BackupExtensionStore.kt`
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BackupExtensionStore {
    /// `@ProtoNumber(1)`
    pub index_url: String,
    /// `@ProtoNumber(2)`
    pub name: String,
    /// `@ProtoNumber(3)`
    pub badge_label: Option<String>,
    /// `@ProtoNumber(4)`
    pub contact_website: String,
    /// `@ProtoNumber(5)`
    pub signing_key: String,
    /// `@ProtoNumber(6)`
    pub contact_discord: Option<String>,
    /// `@ProtoNumber(7)`
    pub is_legacy: Option<bool>,
    /// `@ProtoNumber(8)`
    pub extension_list_url: Option<String>,
}

/// Summary statistics returned after importing a Mihon backup into `Database`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BackupImportReport {
    pub categories_restored: usize,
    pub mangas_restored: usize,
    pub chapters_restored: usize,
    pub history_entries_restored: usize,
    pub tracks_restored: usize,
    pub excluded_scanlators_restored: usize,
    pub extension_stores_restored: usize,
    pub custom_manga_infos_restored: usize,
}

// =============================================================================
// ZERO-PANIC PROTOBUF WIRE PARSER & ENCODER
// =============================================================================

struct ProtoReader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> ProtoReader<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn is_eof(&self) -> bool {
        self.pos >= self.buf.len()
    }

    fn read_varint(&mut self) -> Result<u64> {
        let mut result: u64 = 0;
        let mut shift = 0;
        while shift < 64 {
            if self.pos >= self.buf.len() {
                return Err(IkazuchiError::Parse(
                    "Unexpected EOF while reading Protobuf varint".into(),
                ));
            }
            let byte = self.buf[self.pos];
            self.pos += 1;
            result |= ((byte & 0x7F) as u64) << shift;
            if (byte & 0x80) == 0 {
                return Ok(result);
            }
            shift += 7;
        }
        Err(IkazuchiError::Parse("Malformed Protobuf varint (>10 bytes)".into()))
    }

    fn read_tag(&mut self) -> Result<(u32, u8)> {
        let v = self.read_varint()?;
        let field_number = (v >> 3) as u32;
        let wire_type = (v & 0x07) as u8;
        if field_number == 0 {
            return Err(IkazuchiError::Parse("Invalid Protobuf field number 0".into()));
        }
        Ok((field_number, wire_type))
    }

    fn read_bytes(&mut self) -> Result<&'a [u8]> {
        let len = self.read_varint()? as usize;
        if self.pos.saturating_add(len) > self.buf.len() {
            return Err(IkazuchiError::Parse(
                "Protobuf length-delimited field exceeds buffer".into(),
            ));
        }
        let slice = &self.buf[self.pos..self.pos + len];
        self.pos += len;
        Ok(slice)
    }

    fn read_string(&mut self) -> Result<String> {
        let bytes = self.read_bytes()?;
        Ok(String::from_utf8_lossy(bytes).into_owned())
    }

    fn read_fixed32(&mut self) -> Result<[u8; 4]> {
        if self.pos + 4 > self.buf.len() {
            return Err(IkazuchiError::Parse("Unexpected EOF in fixed32".into()));
        }
        let mut arr = [0u8; 4];
        arr.copy_from_slice(&self.buf[self.pos..self.pos + 4]);
        self.pos += 4;
        Ok(arr)
    }

    fn read_fixed64(&mut self) -> Result<[u8; 8]> {
        if self.pos + 8 > self.buf.len() {
            return Err(IkazuchiError::Parse("Unexpected EOF in fixed64".into()));
        }
        let mut arr = [0u8; 8];
        arr.copy_from_slice(&self.buf[self.pos..self.pos + 8]);
        self.pos += 8;
        Ok(arr)
    }

    fn read_f32_flexible(&mut self, wire_type: u8) -> Result<f32> {
        match wire_type {
            5 => Ok(f32::from_le_bytes(self.read_fixed32()?)),
            1 => Ok(f64::from_le_bytes(self.read_fixed64()?) as f32),
            0 => Ok(self.read_varint()? as i64 as f32),
            _ => {
                self.skip_field(wire_type)?;
                Ok(0.0)
            }
        }
    }

    fn skip_field(&mut self, wire_type: u8) -> Result<()> {
        match wire_type {
            0 => {
                self.read_varint()?;
                Ok(())
            }
            1 => {
                self.read_fixed64()?;
                Ok(())
            }
            2 => {
                self.read_bytes()?;
                Ok(())
            }
            5 => {
                self.read_fixed32()?;
                Ok(())
            }
            other => Err(IkazuchiError::Parse(format!(
                "Unsupported Protobuf wire type {other}"
            ))),
        }
    }
}

/// Decodes a Mihon `.tachibk` / `.proto.gz` or raw `.proto` byte buffer into [`MihonBackup`].
/// Automatically detects Gzip magic bytes (`[0x1F, 0x8B]`).
pub fn decode_mihon_backup_bytes(input: &[u8]) -> Result<MihonBackup> {
    if input.is_empty() {
        return Err(IkazuchiError::Parse("Backup file is empty".into()));
    }

    let raw_proto: Vec<u8> = if input.len() >= 2 && input[0] == 0x1F && input[1] == 0x8B {
        let mut decoder = GzDecoder::new(input);
        let mut out = Vec::new();
        decoder
            .read_to_end(&mut out)
            .map_err(|e| IkazuchiError::Parse(format!("Failed to decompress Gzip backup: {e}")))?;
        out
    } else {
        input.to_vec()
    };

    let mut reader = ProtoReader::new(&raw_proto);
    let mut backup = MihonBackup::default();

    while !reader.is_eof() {
        let (field_num, wire_type) = reader.read_tag()?;
        match (field_num, wire_type) {
            (1, 2) => backup.backup_manga.push(decode_backup_manga(reader.read_bytes()?)?),
            (2, 2) => backup
                .backup_categories
                .push(decode_backup_category(reader.read_bytes()?)?),
            (101, 2) => backup
                .backup_sources
                .push(decode_backup_source(reader.read_bytes()?)?),
            (106, 2) => backup
                .backup_extension_stores
                .push(decode_backup_extension_store(reader.read_bytes()?)?),
            _ => reader.skip_field(wire_type)?,
        }
    }

    Ok(backup)
}

fn decode_backup_manga(bytes: &[u8]) -> Result<BackupManga> {
    let mut r = ProtoReader::new(bytes);
    let mut m = BackupManga::default();

    while !r.is_eof() {
        let (field, wt) = r.read_tag()?;
        match (field, wt) {
            (1, 0) => m.source = r.read_varint()? as i64,
            (2, 2) => m.url = r.read_string()?,
            (3, 2) => m.title = r.read_string()?,
            (4, 2) => m.artist = Some(r.read_string()?),
            (5, 2) => m.author = Some(r.read_string()?),
            (6, 2) => m.description = Some(r.read_string()?),
            (7, 2) => m.genre.push(r.read_string()?),
            (8, 0) => m.status = r.read_varint()? as i32,
            (9, 2) => m.thumbnail_url = Some(r.read_string()?),
            (13, 0) => m.date_added = r.read_varint()? as i64,
            (14, 0) => m.viewer = r.read_varint()? as i32,
            (16, 2) => m.chapters.push(decode_backup_chapter(r.read_bytes()?)?),
            (17, 0) => m.categories.push(r.read_varint()? as i64),
            (17, 2) => {
                // Packed repeated varint support
                let mut sub = ProtoReader::new(r.read_bytes()?);
                while !sub.is_eof() {
                    m.categories.push(sub.read_varint()? as i64);
                }
            }
            (18, 2) => m.tracking.push(decode_backup_tracking(r.read_bytes()?)?),
            (100, 0) => m.favorite = r.read_varint()? != 0,
            (101, 0) => m.chapter_flags = r.read_varint()? as i32,
            (103, 0) => m.viewer_flags = Some(r.read_varint()? as i32),
            (104, 2) => m.history.push(decode_backup_history(r.read_bytes()?)?),
            (105, 0) => m.update_strategy = r.read_varint()? as i32,
            (107, 0) => m.favorite_modified_at = Some(r.read_varint()? as i64),
            (108, 2) => m.excluded_scanlators.push(r.read_string()?),
            (110, 2) => m.notes = r.read_string()?,
            (111, 0) => m.initialized = r.read_varint()? != 0,
            (602, 0) => m.custom_status = r.read_varint()? as i32,
            (800, 2) => m.custom_title = Some(r.read_string()?),
            (801, 2) => m.custom_artist = Some(r.read_string()?),
            (802, 2) => m.custom_author = Some(r.read_string()?),
            (804, 2) => m.custom_description = Some(r.read_string()?),
            (805, 2) => m.custom_genre.push(r.read_string()?),
            _ => r.skip_field(wt)?,
        }
    }
    Ok(m)
}

fn decode_backup_chapter(bytes: &[u8]) -> Result<BackupChapter> {
    let mut r = ProtoReader::new(bytes);
    let mut c = BackupChapter::default();

    while !r.is_eof() {
        let (field, wt) = r.read_tag()?;
        match (field, wt) {
            (1, 2) => c.url = r.read_string()?,
            (2, 2) => c.name = r.read_string()?,
            (3, 2) => c.scanlator = Some(r.read_string()?),
            (4, 0) => c.read = r.read_varint()? != 0,
            (5, 0) => c.bookmark = r.read_varint()? != 0,
            (6, 0) => c.last_page_read = r.read_varint()? as i64,
            (7, 0) => c.date_fetch = r.read_varint()? as i64,
            (8, 0) => c.date_upload = r.read_varint()? as i64,
            (9, _) => c.chapter_number = r.read_f32_flexible(wt)?,
            (10, 0) => c.source_order = r.read_varint()? as i64,
            _ => r.skip_field(wt)?,
        }
    }
    Ok(c)
}

fn decode_backup_category(bytes: &[u8]) -> Result<BackupCategory> {
    let mut r = ProtoReader::new(bytes);
    let mut cat = BackupCategory::default();
    while !r.is_eof() {
        let (field, wt) = r.read_tag()?;
        match (field, wt) {
            (1, 2) => cat.name = r.read_string()?,
            (2, 0) => cat.order = r.read_varint()? as i64,
            (3, 0) => cat.id = r.read_varint()? as i64,
            (100, 0) => cat.flags = r.read_varint()? as i64,
            _ => r.skip_field(wt)?,
        }
    }
    Ok(cat)
}

fn decode_backup_history(bytes: &[u8]) -> Result<BackupHistory> {
    let mut r = ProtoReader::new(bytes);
    let mut h = BackupHistory::default();
    while !r.is_eof() {
        let (field, wt) = r.read_tag()?;
        match (field, wt) {
            (1, 2) => h.url = r.read_string()?,
            (2, 0) => h.last_read = r.read_varint()? as i64,
            (3, 0) => h.read_duration = r.read_varint()? as i64,
            _ => r.skip_field(wt)?,
        }
    }
    Ok(h)
}

fn decode_backup_tracking(bytes: &[u8]) -> Result<BackupTracking> {
    let mut r = ProtoReader::new(bytes);
    let mut t = BackupTracking::default();
    while !r.is_eof() {
        let (field, wt) = r.read_tag()?;
        match (field, wt) {
            (1, 0) => t.sync_id = r.read_varint()? as i32,
            (2, 0) => t.library_id = r.read_varint()? as i64,
            (3, 0) => t.media_id_int = r.read_varint()? as i32,
            (4, 2) => t.tracking_url = r.read_string()?,
            (5, 2) => t.title = r.read_string()?,
            (6, _) => t.last_chapter_read = r.read_f32_flexible(wt)?,
            (7, 0) => t.total_chapters = r.read_varint()? as i32,
            (8, _) => t.score = r.read_f32_flexible(wt)?,
            (9, 0) => t.status = r.read_varint()? as i32,
            (10, 0) => t.started_reading_date = r.read_varint()? as i64,
            (11, 0) => t.finished_reading_date = r.read_varint()? as i64,
            (12, 0) => t.private = r.read_varint()? != 0,
            (100, 0) => t.media_id = r.read_varint()? as i64,
            _ => r.skip_field(wt)?,
        }
    }
    Ok(t)
}

fn decode_backup_source(bytes: &[u8]) -> Result<BackupSource> {
    let mut r = ProtoReader::new(bytes);
    let mut s = BackupSource::default();
    while !r.is_eof() {
        let (field, wt) = r.read_tag()?;
        match (field, wt) {
            (1, 2) => s.name = r.read_string()?,
            (2, 0) => s.source_id = r.read_varint()? as i64,
            _ => r.skip_field(wt)?,
        }
    }
    Ok(s)
}

fn decode_backup_extension_store(bytes: &[u8]) -> Result<BackupExtensionStore> {
    let mut r = ProtoReader::new(bytes);
    let mut es = BackupExtensionStore::default();
    while !r.is_eof() {
        let (field, wt) = r.read_tag()?;
        match (field, wt) {
            (1, 2) => es.index_url = r.read_string()?,
            (2, 2) => es.name = r.read_string()?,
            (3, 2) => es.badge_label = Some(r.read_string()?),
            (4, 2) => es.contact_website = r.read_string()?,
            (5, 2) => es.signing_key = r.read_string()?,
            (6, 2) => es.contact_discord = Some(r.read_string()?),
            (7, 0) => es.is_legacy = Some(r.read_varint()? != 0),
            (8, 2) => es.extension_list_url = Some(r.read_string()?),
            _ => r.skip_field(wt)?,
        }
    }
    Ok(es)
}

// =============================================================================
// PROTOBUF ENCODER (FOR EXPORTING MIHON .tachibk BACKUPS)
// =============================================================================

fn write_varint(out: &mut Vec<u8>, mut val: u64) {
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

fn write_tag(out: &mut Vec<u8>, field_num: u32, wire_type: u8) {
    write_varint(out, ((field_num as u64) << 3) | (wire_type as u64));
}

fn write_varint_field(out: &mut Vec<u8>, field_num: u32, val: u64) {
    write_tag(out, field_num, 0);
    write_varint(out, val);
}

fn write_f32_field(out: &mut Vec<u8>, field_num: u32, val: f32) {
    write_tag(out, field_num, 5);
    out.extend_from_slice(&val.to_le_bytes());
}

fn write_bytes_field(out: &mut Vec<u8>, field_num: u32, bytes: &[u8]) {
    write_tag(out, field_num, 2);
    write_varint(out, bytes.len() as u64);
    out.extend_from_slice(bytes);
}

fn write_string_field(out: &mut Vec<u8>, field_num: u32, s: &str) {
    write_bytes_field(out, field_num, s.as_bytes());
}

/// Encodes a [`MihonBackup`] into a Mihon-compatible Protobuf stream (optionally Gzip-compressed as `.tachibk`).
pub fn encode_mihon_backup_bytes(backup: &MihonBackup, gzip: bool) -> Result<Vec<u8>> {
    let mut proto = Vec::new();

    for m in &backup.backup_manga {
        let mut mb = Vec::new();
        write_varint_field(&mut mb, 1, m.source as u64);
        write_string_field(&mut mb, 2, &m.url);
        write_string_field(&mut mb, 3, &m.title);
        if let Some(artist) = &m.artist {
            write_string_field(&mut mb, 4, artist);
        }
        if let Some(author) = &m.author {
            write_string_field(&mut mb, 5, author);
        }
        if let Some(desc) = &m.description {
            write_string_field(&mut mb, 6, desc);
        }
        for g in &m.genre {
            write_string_field(&mut mb, 7, g);
        }
        write_varint_field(&mut mb, 8, m.status as u64);
        if let Some(thumb) = &m.thumbnail_url {
            write_string_field(&mut mb, 9, thumb);
        }
        if m.date_added != 0 {
            write_varint_field(&mut mb, 13, m.date_added as u64);
        }
        if m.viewer != 0 {
            write_varint_field(&mut mb, 14, m.viewer as u64);
        }
        for c in &m.chapters {
            let mut cb = Vec::new();
            write_string_field(&mut cb, 1, &c.url);
            write_string_field(&mut cb, 2, &c.name);
            if let Some(sc) = &c.scanlator {
                write_string_field(&mut cb, 3, sc);
            }
            write_varint_field(&mut cb, 4, u64::from(c.read));
            write_varint_field(&mut cb, 5, u64::from(c.bookmark));
            write_varint_field(&mut cb, 6, c.last_page_read as u64);
            write_varint_field(&mut cb, 7, c.date_fetch as u64);
            write_varint_field(&mut cb, 8, c.date_upload as u64);
            write_f32_field(&mut cb, 9, c.chapter_number);
            write_varint_field(&mut cb, 10, c.source_order as u64);
            write_bytes_field(&mut mb, 16, &cb);
        }
        for &cat_order in &m.categories {
            write_varint_field(&mut mb, 17, cat_order as u64);
        }
        for t in &m.tracking {
            let mut tb = Vec::new();
            write_varint_field(&mut tb, 1, t.sync_id as u64);
            write_varint_field(&mut tb, 2, t.library_id as u64);
            if t.media_id_int != 0 {
                write_varint_field(&mut tb, 3, t.media_id_int as u64);
            }
            if !t.tracking_url.is_empty() {
                write_string_field(&mut tb, 4, &t.tracking_url);
            }
            if !t.title.is_empty() {
                write_string_field(&mut tb, 5, &t.title);
            }
            write_f32_field(&mut tb, 6, t.last_chapter_read);
            write_varint_field(&mut tb, 7, t.total_chapters as u64);
            write_f32_field(&mut tb, 8, t.score);
            write_varint_field(&mut tb, 9, t.status as u64);
            write_varint_field(&mut tb, 10, t.started_reading_date as u64);
            write_varint_field(&mut tb, 11, t.finished_reading_date as u64);
            write_varint_field(&mut tb, 12, u64::from(t.private));
            write_varint_field(&mut tb, 100, t.media_id as u64);
            write_bytes_field(&mut mb, 18, &tb);
        }
        write_varint_field(&mut mb, 100, u64::from(m.favorite));
        write_varint_field(&mut mb, 101, m.chapter_flags as u64);
        if let Some(vf) = m.viewer_flags {
            write_varint_field(&mut mb, 103, vf as u64);
        }
        for h in &m.history {
            let mut hb = Vec::new();
            write_string_field(&mut hb, 1, &h.url);
            write_varint_field(&mut hb, 2, h.last_read as u64);
            write_varint_field(&mut hb, 3, h.read_duration as u64);
            write_bytes_field(&mut mb, 104, &hb);
        }
        write_varint_field(&mut mb, 105, m.update_strategy as u64);
        if let Some(fmod) = m.favorite_modified_at {
            write_varint_field(&mut mb, 107, fmod as u64);
        }
        for scanlator in &m.excluded_scanlators {
            write_string_field(&mut mb, 108, scanlator);
        }
        if !m.notes.is_empty() {
            write_string_field(&mut mb, 110, &m.notes);
        }
        write_varint_field(&mut mb, 111, u64::from(m.initialized));

        // Yokai / J2K / TachiyomiSY custom metadata tags (602, 800..805)
        if m.custom_status != 0 {
            write_varint_field(&mut mb, 602, m.custom_status as u64);
        }
        if let Some(ct) = &m.custom_title {
            write_string_field(&mut mb, 800, ct);
        }
        if let Some(ca) = &m.custom_artist {
            write_string_field(&mut mb, 801, ca);
        }
        if let Some(cau) = &m.custom_author {
            write_string_field(&mut mb, 802, cau);
        }
        if let Some(cd) = &m.custom_description {
            write_string_field(&mut mb, 804, cd);
        }
        for cg in &m.custom_genre {
            write_string_field(&mut mb, 805, cg);
        }

        write_bytes_field(&mut proto, 1, &mb);
    }

    for cat in &backup.backup_categories {
        let mut cb = Vec::new();
        write_string_field(&mut cb, 1, &cat.name);
        write_varint_field(&mut cb, 2, cat.order as u64);
        write_varint_field(&mut cb, 3, cat.id as u64);
        write_varint_field(&mut cb, 100, cat.flags as u64);
        write_bytes_field(&mut proto, 2, &cb);
    }

    for src in &backup.backup_sources {
        let mut sb = Vec::new();
        write_string_field(&mut sb, 1, &src.name);
        write_varint_field(&mut sb, 2, src.source_id as u64);
        write_bytes_field(&mut proto, 101, &sb);
    }

    for es in &backup.backup_extension_stores {
        let mut eb = Vec::new();
        write_string_field(&mut eb, 1, &es.index_url);
        write_string_field(&mut eb, 2, &es.name);
        if let Some(bl) = &es.badge_label {
            write_string_field(&mut eb, 3, bl);
        }
        write_string_field(&mut eb, 4, &es.contact_website);
        write_string_field(&mut eb, 5, &es.signing_key);
        if let Some(cd) = &es.contact_discord {
            write_string_field(&mut eb, 6, cd);
        }
        if let Some(leg) = es.is_legacy {
            write_varint_field(&mut eb, 7, u64::from(leg));
        }
        if let Some(elu) = &es.extension_list_url {
            write_string_field(&mut eb, 8, elu);
        }
        write_bytes_field(&mut proto, 106, &eb);
    }

    if gzip {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder
            .write_all(&proto)
            .map_err(|e| IkazuchiError::Parse(e.to_string()))?;
        encoder
            .finish()
            .map_err(|e| IkazuchiError::Parse(e.to_string()))
    } else {
        Ok(proto)
    }
}

fn normalize_backup_url(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        url_without_domain(trimmed)
    } else {
        trimmed.to_string()
    }
}

impl Database {
    /// Restores a Mihon `.tachibk` / `.proto.gz` (or raw `.proto`) backup into SQLite with full
    /// Mihon & fork safeguards (`RestoreRepositoryImpl.kt` + `MangaRestorer.kt` parity).
    pub fn import_mihon_backup_bytes(&self, bytes: &[u8], now_ms: i64) -> Result<BackupImportReport> {
        let backup = decode_mihon_backup_bytes(bytes)?;
        self.restore_mihon_backup(&backup, now_ms)
    }

    /// Restores a decoded [`MihonBackup`] into SQLite idempotently.
    pub fn restore_mihon_backup(
        &self,
        backup: &MihonBackup,
        now_ms: i64,
    ) -> Result<BackupImportReport> {
        let mut report = BackupImportReport::default();

        // 1. Restore Extension Stores (`BackupExtensionStore`)
        for es in &backup.backup_extension_stores {
            if !es.index_url.trim().is_empty() {
                self.upsert_extension_store(&es.index_url, &es.name, &es.signing_key)?;
                report.extension_stores_restored += 1;
            }
        }

        // 2. Restore Categories (`BackupCategory`) & build 3-tier lookup map
        // Safeguard #2: Support matching by `order`, `id`, or 0-based index!
        let mut by_order: HashMap<i64, String> = HashMap::new();
        let mut by_id: HashMap<i64, String> = HashMap::new();
        let mut by_index: Vec<String> = Vec::with_capacity(backup.backup_categories.len());

        let mut sorted_cats = backup.backup_categories.clone();
        sorted_cats.sort_by_key(|c| c.order);
        for cat in &sorted_cats {
            if !cat.name.trim().is_empty() {
                self.create_category(&cat.name, cat.flags as i32, now_ms)?;
                report.categories_restored += 1;
            }
        }

        let distinct_orders: std::collections::HashSet<i64> =
            backup.backup_categories.iter().map(|c| c.order).collect();
        let has_distinct_orders =
            distinct_orders.len() == backup.backup_categories.len() && !distinct_orders.is_empty();

        for cat in &backup.backup_categories {
            by_order.insert(cat.order, cat.name.clone());
            if cat.id > 0 {
                by_id.insert(cat.id, cat.name.clone());
            }
            by_index.push(cat.name.clone());
        }

        // 3. Restore Mangas, Chapters, Categories, History, Trackers, and Excluded Scanlators
        for bm in &backup.backup_manga {
            let norm_manga_url = normalize_backup_url(&bm.url);
            if norm_manga_url.is_empty() {
                continue;
            }

            // Safeguard #1 & #4: Compute `favorite_at` in milliseconds
            let fav_at: Option<i64> = if bm.favorite {
                let candidate = if bm.date_added > 0 {
                    bm.date_added
                } else if let Some(sec) = bm.favorite_modified_at.filter(|&s| s > 0) {
                    sec.saturating_mul(1000)
                } else {
                    now_ms.max(1)
                };
                // If existing manga is already favorited earlier, keep earliest non-zero timestamp
                if let Some(existing) = self.get_manga_by_source_and_url(bm.source, &norm_manga_url)? {
                    if existing.favorite_at > 0 {
                        Some(existing.favorite_at.min(candidate))
                    } else {
                        Some(candidate)
                    }
                } else {
                    Some(candidate)
                }
            } else if let Some(existing) = self.get_manga_by_source_and_url(bm.source, &norm_manga_url)? {
                // Never unfavorite an already-favorited local manga when merging a backup!
                if existing.favorite_at > 0 {
                    Some(existing.favorite_at)
                } else {
                    None
                }
            } else {
                None
            };

            let smanga = SManga {
                url: norm_manga_url,
                title: bm.title.clone(),
                author: bm.author.clone(),
                artist: bm.artist.clone(),
                description: bm.description.clone(),
                genre: bm.genre.clone(),
                status: MangaStatus::from(bm.status),
                thumbnail_url: bm.thumbnail_url.clone(),
                update_strategy: UpdateStrategy::from(bm.update_strategy),
                initialized: bm.initialized || !bm.title.is_empty(),
            };

            let manga_id = self.upsert_manga(bm.source, &smanga, fav_at, now_ms)?;
            let reader_flags = bm.viewer_flags.unwrap_or(bm.viewer);
            if reader_flags != 0 {
                self.set_manga_reader_flags(manga_id, reader_flags, now_ms)?;
            }
            report.mangas_restored += 1;

            // Resolve categories using 3-tier lookup and merge with any existing categories on `manga_id`
            if !bm.categories.is_empty() {
                let mut cat_names: Vec<String> = self
                    .get_categories_for_manga(manga_id)?
                    .into_iter()
                    .map(|c| c.name)
                    .collect();

                for &cat_ref in &bm.categories {
                    let resolved = if has_distinct_orders {
                        by_order
                            .get(&cat_ref)
                            .or_else(|| by_id.get(&cat_ref))
                            .or_else(|| by_index.get(cat_ref as usize))
                    } else {
                        by_id
                            .get(&cat_ref)
                            .or_else(|| by_index.get(cat_ref as usize))
                            .or_else(|| by_order.get(&cat_ref))
                    };
                    if let Some(name) = resolved {
                        if !name.trim().is_empty() && !cat_names.contains(name) {
                            cat_names.push(name.clone());
                        }
                    }
                }
                self.set_manga_categories_by_name(manga_id, &cat_names, now_ms)?;
            }

            // Safeguard #3 & #4: Coalesce duplicate chapters in backup by normalized URL
            // and clamp `last_page_read` to `0..=i32::MAX`
            let mut coalesced_chapters: BTreeMap<String, BackupChapter> = BTreeMap::new();
            for ch in &bm.chapters {
                let norm_ch_url = normalize_backup_url(&ch.url);
                if norm_ch_url.is_empty() {
                    continue;
                }
                let clamped_page = ch.last_page_read.clamp(0, i32::MAX as i64);
                coalesced_chapters
                    .entry(norm_ch_url.clone())
                    .and_modify(|existing| {
                        existing.read |= ch.read;
                        existing.bookmark |= ch.bookmark;
                        existing.last_page_read = existing.last_page_read.max(clamped_page);
                        existing.date_fetch = existing.date_fetch.max(ch.date_fetch);
                        existing.date_upload = existing.date_upload.max(ch.date_upload);
                    })
                    .or_insert_with(|| {
                        let mut clean = ch.clone();
                        clean.url = norm_ch_url;
                        clean.last_page_read = clamped_page;
                        clean
                    });
            }

            if !coalesced_chapters.is_empty() {
                let mut ordered_bchs: Vec<BackupChapter> = coalesced_chapters.into_values().collect();
                ordered_bchs.sort_by(|a, b| {
                    a.source_order
                        .cmp(&b.source_order)
                        .then_with(|| b.chapter_number.total_cmp(&a.chapter_number))
                });

                let schapters: Vec<SChapter> = ordered_bchs
                    .iter()
                    .map(|c| SChapter {
                        url: c.url.clone(),
                        name: if c.name.is_empty() {
                            format!("Ch. {}", c.chapter_number)
                        } else {
                            c.name.clone()
                        },
                        chapter_number: c.chapter_number,
                        date_upload: c.date_upload,
                        scanlator: c.scanlator.clone(),
                    })
                    .collect();

                self.sync_chapters(manga_id, &schapters, now_ms)?;

                // Merge read/bookmark/last_page_read with existing DB state (`read || db.read`, `max(page)`)
                let db_chapters = self.get_chapters_by_manga_id(manga_id)?;
                let db_by_url: HashMap<String, _> = db_chapters
                    .into_iter()
                    .map(|c| (c.chapter.url.clone(), c))
                    .collect();

                for bch in &ordered_bchs {
                    if let Some(db_ch) = db_by_url.get(&bch.url) {
                        let merged_read = db_ch.read || bch.read;
                        let merged_bookmark = db_ch.bookmark || bch.bookmark;
                        let merged_page = db_ch.last_page_read.max(bch.last_page_read as u32);
                        if merged_read != db_ch.read
                            || merged_bookmark != db_ch.bookmark
                            || merged_page != db_ch.last_page_read
                        {
                            let ts = if bch.date_fetch > 0 { bch.date_fetch } else { now_ms };
                            self.update_chapter_progress(
                                db_ch.id,
                                merged_read,
                                merged_bookmark,
                                merged_page,
                                ts,
                            )?;
                        }
                        report.chapters_restored += 1;
                    }
                }
            }

            // Restore History entries (`BackupHistory`)
            if !bm.history.is_empty() {
                let db_chapters = self.get_chapters_by_manga_id(manga_id)?;
                let ch_id_by_url: HashMap<String, i64> = db_chapters
                    .into_iter()
                    .map(|c| (c.chapter.url, c.id))
                    .collect();

                for bh in &bm.history {
                    let norm_url = normalize_backup_url(&bh.url);
                    if let Some(&ch_id) = ch_id_by_url.get(&norm_url) {
                        if bh.last_read > 0 {
                            self.upsert_history_entry(ch_id, bh.last_read, bh.read_duration.max(0))?;
                            report.history_entries_restored += 1;
                        }
                    }
                }
            }

            // Restore Trackers (`BackupTracking`)
            for bt in &bm.tracking {
                let track = MangaTrackRecord {
                    id: 0,
                    manga_id,
                    tracker_id: bt.sync_id as i64,
                    remote_id: bt.resolved_remote_id(),
                    library_id: Some(bt.library_id),
                    title: bt.title.clone(),
                    last_chapter_read: bt.last_chapter_read as f64,
                    total_chapters: bt.total_chapters as i64,
                    status: bt.status as i64,
                    score: bt.score as f64,
                    remote_url: bt.tracking_url.clone(),
                    start_date: bt.started_reading_date,
                    finish_date: bt.finished_reading_date,
                    private: bt.private,
                };
                self.upsert_manga_track(&track)?;
                report.tracks_restored += 1;
            }

            // Restore Excluded Scanlators (`excludedScanlators`)
            for scanlator in &bm.excluded_scanlators {
                if !scanlator.trim().is_empty() {
                    self.add_excluded_scanlator(manga_id, scanlator)?;
                    report.excluded_scanlators_restored += 1;
                }
            }

            // Restore Yokai / J2K / TachiyomiSY Custom Manga Info (`custom_manga_info`, tags 602, 800..805)
            let custom_info = CustomMangaInfo {
                title: bm.custom_title.clone().filter(|s| !s.trim().is_empty()),
                author: bm.custom_author.clone().filter(|s| !s.trim().is_empty()),
                artist: bm.custom_artist.clone().filter(|s| !s.trim().is_empty()),
                description: bm
                    .custom_description
                    .clone()
                    .filter(|s| !s.trim().is_empty()),
                genre: if bm.custom_genre.is_empty() {
                    None
                } else {
                    Some(bm.custom_genre.clone())
                },
                status: if bm.custom_status != 0 {
                    Some(MangaStatus::from(bm.custom_status))
                } else {
                    None
                },
            };
            if !custom_info.is_empty() {
                self.set_custom_manga_info(manga_id, &custom_info)?;
                report.custom_manga_infos_restored += 1;
            }
        }

        Ok(report)
    }

    /// Exports the local SQLite library into a Mihon-compatible `.tachibk` (`.proto.gz`) or `.proto` byte buffer.
    pub fn export_mihon_backup_bytes(&self, gzip: bool) -> Result<Vec<u8>> {
        let categories = self.get_categories()?;
        let mut cat_order_by_name: HashMap<String, i64> = HashMap::new();
        let backup_categories: Vec<BackupCategory> = categories
            .into_iter()
            .map(|c| {
                cat_order_by_name.insert(c.name.clone(), c.sort_order as i64);
                BackupCategory {
                    name: c.name,
                    order: c.sort_order as i64,
                    id: c.id,
                    flags: c.flags as i64,
                }
            })
            .collect();

        let library = self.get_library_manga()?;
        let mut backup_manga = Vec::with_capacity(library.len());

        for item in library {
            let db_chapters = self.get_chapters_by_manga_id(item.id)?;
            let chapters: Vec<BackupChapter> = db_chapters
                .into_iter()
                .map(|c| BackupChapter {
                    url: c.chapter.url,
                    name: c.chapter.name,
                    scanlator: c.chapter.scanlator,
                    read: c.read,
                    bookmark: c.bookmark,
                    last_page_read: c.last_page_read as i64,
                    date_fetch: c.sync_updated_at,
                    date_upload: c.chapter.date_upload,
                    chapter_number: c.chapter.chapter_number,
                    source_order: c.source_order as i64,
                })
                .collect();

            let cat_orders: Vec<i64> = item
                .categories
                .iter()
                .filter_map(|name| cat_order_by_name.get(name).copied())
                .collect();

            let tracks: Vec<BackupTracking> = self
                .get_tracks_for_manga(item.id)?
                .into_iter()
                .map(|t| BackupTracking {
                    sync_id: t.tracker_id as i32,
                    library_id: t.library_id.unwrap_or(0),
                    media_id_int: 0,
                    tracking_url: t.remote_url,
                    title: t.title,
                    last_chapter_read: t.last_chapter_read as f32,
                    total_chapters: t.total_chapters as i32,
                    score: t.score as f32,
                    status: t.status as i32,
                    started_reading_date: t.start_date,
                    finished_reading_date: t.finish_date,
                    private: t.private,
                    media_id: t.remote_id,
                })
                .collect();

            let excluded_scanlators = self.get_excluded_scanlators(item.id)?;
            let history = self.get_history_for_manga(item.id)?;
            let custom = self.get_custom_manga_info(item.id)?.unwrap_or_default();

            backup_manga.push(BackupManga {
                source: item.source_id,
                url: item.manga.url,
                title: item.manga.title,
                artist: item.manga.artist,
                author: item.manga.author,
                description: item.manga.description,
                genre: item.manga.genre,
                status: item.manga.status as i32,
                thumbnail_url: item.manga.thumbnail_url,
                date_added: item.favorite_at,
                viewer: item.reader_flags,
                chapters,
                categories: cat_orders,
                tracking: tracks,
                favorite: true,
                chapter_flags: 0,
                viewer_flags: Some(item.reader_flags),
                history,
                update_strategy: item.manga.update_strategy as i32,
                favorite_modified_at: None,
                excluded_scanlators,
                notes: String::new(),
                initialized: item.manga.initialized,
                custom_status: custom.status.map(|s| s as i32).unwrap_or(0),
                custom_title: custom.title,
                custom_artist: custom.artist,
                custom_author: custom.author,
                custom_description: custom.description,
                custom_genre: custom.genre.unwrap_or_default(),
            });
        }

        let backup_extension_stores: Vec<BackupExtensionStore> = self
            .get_extension_stores()?
            .into_iter()
            .map(|es| BackupExtensionStore {
                index_url: es.index_url,
                name: es.name,
                badge_label: None,
                contact_website: String::new(),
                signing_key: es.signing_key,
                contact_discord: None,
                is_legacy: Some(false),
                extension_list_url: None,
            })
            .collect();

        let backup = MihonBackup {
            backup_manga,
            backup_categories,
            backup_sources: Vec::new(),
            backup_extension_stores,
        };

        encode_mihon_backup_bytes(&backup, gzip)
    }
}
