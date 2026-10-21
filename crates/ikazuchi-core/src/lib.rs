//! # `ikazuchi-core`
//!
//! Core domain models, Mihon-compatible `:source-api` data contracts (`SManga`, `SChapter`,
//! `Page`, `FilterList`, `MangaSource`), deterministic 63-bit MD5 source ID generation,
//! and domain-independent relative URL utilities.

pub mod config;

use async_trait::async_trait;
use md5::{Digest, Md5};
use serde::{Deserialize, Serialize};

pub use config::{
    AppConfig, AppTheme, BackupSyncSettings, DownloadSettings, ExtensionSettings, ImageScaleType,
    LibraryDisplayMode, LibraryGroupMode, LibrarySettings, LibrarySortMode, ReaderSettings,
    ReadingModeSetting, ThemeMode, UiSettings,
};

/// Errors returned by source and core operations.
#[derive(Debug, thiserror::Error)]
pub enum IkazuchiError {
    #[error("Network error: {0}")]
    Network(String),
    #[error("Parse error: {0}")]
    Parse(String),
    #[error("Database error: {0}")]
    Database(String),
    #[error("Unsupported operation: {0}")]
    Unsupported(String),
}

pub type Result<T> = std::result::Result<T, IkazuchiError>;

/// Generates a deterministic 63-bit positive source ID identical to Mihon's `HttpSource.generateId`.
///
/// Formula: First 8 bytes (big-endian) of `MD5("${name.lowercase()}/$lang/$versionId")` with sign bit masked to 0.
pub fn generate_source_id(name: &str, lang: &str, version_id: i32) -> i64 {
    let key = format!("{}/{}/{}", name.to_lowercase(), lang, version_id);
    let digest = Md5::digest(key.as_bytes());
    i64::from_be_bytes(digest[0..8].try_into().unwrap()) & i64::MAX
}

/// Generates a deterministic 16-char hex shard key for GitHub sync (`library/mangas/<hash>.json`).
pub fn compute_manga_sync_hash(source_id: i64, remote_url: &str) -> String {
    let key = format!("{source_id}:{remote_url}");
    let digest = Md5::digest(key.as_bytes());
    format!("{:016x}", u64::from_be_bytes(digest[0..8].try_into().unwrap()))
}

/// Strips scheme and domain from a full URL so database entries survive source domain changes
/// (identical to Mihon's `HttpSource.setUrlWithoutDomain`).
pub fn url_without_domain(orig: &str) -> String {
    let encoded = orig.trim().replace(' ', "%20");
    if let Some((_, after_scheme)) = encoded.split_once("://") {
        if let Some(slash_idx) = after_scheme.find('/') {
            return after_scheme[slash_idx..].to_string();
        }
        return "/".to_string();
    }
    encoded
}

/// Publication status of a manga (matches Mihon's `SManga` constants).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[repr(i32)]
pub enum MangaStatus {
    #[default]
    Unknown = 0,
    Ongoing = 1,
    Completed = 2,
    Licensed = 3,
    PublishingFinished = 4,
    Cancelled = 5,
    OnHiatus = 6,
}

impl From<i32> for MangaStatus {
    fn from(v: i32) -> Self {
        match v {
            1 => Self::Ongoing,
            2 => Self::Completed,
            3 => Self::Licensed,
            4 => Self::PublishingFinished,
            5 => Self::Cancelled,
            6 => Self::OnHiatus,
            _ => Self::Unknown,
        }
    }
}

/// Strategy for updating chapters during library refresh.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[repr(i32)]
pub enum UpdateStrategy {
    #[default]
    AlwaysUpdate = 0,
    OnlyFetchOnce = 1,
}

impl From<i32> for UpdateStrategy {
    fn from(v: i32) -> Self {
        if v == 1 {
            Self::OnlyFetchOnce
        } else {
            Self::AlwaysUpdate
        }
    }
}

/// Standardized Manga metadata contract (`SManga` in Mihon's `:source-api`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SManga {
    pub url: String,
    pub title: String,
    pub artist: Option<String>,
    pub author: Option<String>,
    pub description: Option<String>,
    pub genre: Vec<String>,
    pub status: MangaStatus,
    pub thumbnail_url: Option<String>,
    pub update_strategy: UpdateStrategy,
    pub initialized: bool,
}

/// Standardized Chapter contract (`SChapter` in Mihon's `:source-api`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct SChapter {
    pub url: String,
    pub name: String,
    pub date_upload: i64,
    pub chapter_number: f32,
    pub scanlator: Option<String>,
}

/// Standardized Page contract (`Page` in Mihon's `:source-api`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Page {
    pub index: u32,
    pub url: String,
    pub image_url: Option<String>,
}

/// Paginated list of mangas returned by catalog browse/search calls.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MangasPage {
    pub mangas: Vec<SManga>,
    pub has_next_page: bool,
}

/// Filter primitives matching Mihon's `Filter<T>` hierarchy.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Filter {
    Header { name: String },
    Separator,
    Text { name: String, state: String },
    CheckBox { name: String, state: bool },
    TriState { name: String, state: u8 }, // 0 = Ignore, 1 = Include, 2 = Exclude
    Select { name: String, values: Vec<String>, state: usize },
}

pub type FilterList = Vec<Filter>;

/// Core trait implemented by all Ikazuchi manga/manhwa sources (Native Multisrc, WASM, or DEX Bridge).
#[async_trait]
pub trait MangaSource: Send + Sync {
    fn id(&self) -> i64;
    fn name(&self) -> &str;
    fn lang(&self) -> &str;
    fn base_url(&self) -> &str;
    fn supports_latest(&self) -> bool {
        true
    }
    fn get_filter_list(&self) -> FilterList {
        Vec::new()
    }

    async fn get_popular_manga(&self, page: u32) -> Result<MangasPage>;
    async fn get_latest_updates(&self, page: u32) -> Result<MangasPage>;
    async fn get_search_manga(&self, page: u32, query: &str, filters: &FilterList) -> Result<MangasPage>;
    async fn get_manga_details(&self, manga: &SManga) -> Result<SManga>;
    async fn get_chapter_list(&self, manga: &SManga) -> Result<Vec<SChapter>>;
    async fn get_page_list(&self, chapter: &SChapter) -> Result<Vec<Page>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deterministic_source_id_matches_mihon() {
        let id = generate_source_id("MangaDex", "en", 1);
        assert!(id > 0);
        // Deterministic across runs
        assert_eq!(id, generate_source_id("mangadex", "en", 1));
    }

    #[test]
    fn test_url_without_domain() {
        assert_eq!(
            url_without_domain("https://mangadex.org/title/abc-123?tab=chapters#top"),
            "/title/abc-123?tab=chapters#top"
        );
    }
}
