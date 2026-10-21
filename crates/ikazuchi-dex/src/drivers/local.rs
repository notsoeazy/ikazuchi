//! # Local Filesystem Source (`LocalSource`, `source_id = 0`)
//!
//! Compatible with Mihon's `LocalSource.ID = 0L`:
//! - Scans `<root_dir>/<Manga Title>/` folders for `.cbz` and `.zip` chapter archives.
//! - Extracts canonical chapter numbers via [`crate::chapter_num::parse_chapter_number`].
//! - Generates `ikazuchi://localhost/cbz/<index>?path=<cbz_path>` streaming URLs for `fetch_page_list`.

use crate::chapter_num::parse_chapter_number;
use async_trait::async_trait;
use ikazuchi_archive::{ComicInfoMetadata, list_cbz_pages, read_cbz_comic_info};
use ikazuchi_core::{
    FilterList, IkazuchiError, MangaSource, MangaStatus, MangasPage, Page, Result, SChapter, SManga,
};
use std::path::{Path, PathBuf};

/// Mihon's reserved source ID for `LocalSource`.
pub const LOCAL_SOURCE_ID: i64 = 0;

#[derive(Debug, Clone)]
pub struct LocalSource {
    pub root_dir: PathBuf,
}

impl LocalSource {
    pub fn new(root_dir: impl Into<PathBuf>) -> Self {
        Self {
            root_dir: root_dir.into(),
        }
    }

    /// Scans `self.root_dir` for manga subdirectories (each containing `.cbz`/`.zip` files).
    pub fn scan_local_mangas(&self, query: Option<&str>) -> Result<Vec<SManga>> {
        if !self.root_dir.exists() {
            return Ok(Vec::new());
        }

        let q_lower = query.map(|q| q.trim().to_lowercase()).unwrap_or_default();
        let mut mangas = Vec::new();

        let entries = std::fs::read_dir(&self.root_dir)
            .map_err(|e| IkazuchiError::Parse(format!("Failed to read local source dir: {e}")))?;

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let Some(folder_name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if folder_name.starts_with('.') {
                continue;
            }
            let cover_opt = find_local_cover(&path);
            let mut smanga = SManga {
                url: folder_name.to_string(),
                title: folder_name.to_string(),
                author: None,
                artist: None,
                description: Some(format!("Local manga archive in '{}'", folder_name)),
                genre: vec!["Local".to_string()],
                status: MangaStatus::Unknown,
                thumbnail_url: cover_opt,
                update_strategy: Default::default(),
                initialized: true,
            };
            apply_dir_comic_info(&path, &mut smanga);

            if !q_lower.is_empty()
                && !folder_name.to_lowercase().contains(&q_lower)
                && !smanga.title.to_lowercase().contains(&q_lower)
            {
                continue;
            }
            mangas.push(smanga);
        }

        mangas.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));
        Ok(mangas)
    }

    /// Resolves a relative manga or chapter URL securely inside `self.root_dir`,
    /// rejecting any path traversal (`..`, root `/`, Windows drive prefix) attempts via `std::path::Component`.
    fn resolve_safe_path(&self, rel_url: &str) -> Result<PathBuf> {
        let normalized = rel_url.trim_start_matches(['/', '\\']).replace('\\', "/");
        if Path::new(&normalized).components().any(|c| {
            matches!(
                c,
                std::path::Component::ParentDir
                    | std::path::Component::RootDir
                    | std::path::Component::Prefix(_)
            )
        }) {
            return Err(IkazuchiError::Parse(
                "Path traversal ('..') rejected in LocalSource URL".into(),
            ));
        }
        Ok(self.root_dir.join(normalized))
    }
}

fn find_local_cover(manga_dir: &Path) -> Option<String> {
    for name in ["cover.jpg", "cover.jpeg", "cover.png", "cover.webp"] {
        let p = manga_dir.join(name);
        if p.exists() {
            return Some(p.display().to_string());
        }
    }
    None
}

/// Applies metadata from `<manga_dir>/ComicInfo.xml` if present (Yokai / Mihon `LocalSource.kt` parity).
fn apply_dir_comic_info(manga_dir: &Path, manga: &mut SManga) {
    let comic_info_path = manga_dir.join("ComicInfo.xml");
    let Ok(xml_str) = std::fs::read_to_string(&comic_info_path) else {
        return;
    };
    let info = ComicInfoMetadata::from_xml(&xml_str);
    if let Some(series) = info.series.filter(|s| !s.is_empty()) {
        manga.title = series;
    }
    if let Some(writer) = info.writer.filter(|s| !s.is_empty()) {
        manga.author = Some(writer);
    }
    if let Some(penciller) = info.penciller.filter(|s| !s.is_empty()) {
        manga.artist = Some(penciller);
    }
    if let Some(summary) = info.summary.filter(|s| !s.is_empty()) {
        manga.description = Some(summary);
    }
    if !info.genre.is_empty() {
        manga.genre = info.genre;
    }
    if let Some(st) = info.publishing_status {
        manga.status = st;
    }
}

#[async_trait]
impl MangaSource for LocalSource {
    fn id(&self) -> i64 {
        LOCAL_SOURCE_ID
    }

    fn name(&self) -> &str {
        "Local Source"
    }

    fn lang(&self) -> &str {
        "all"
    }

    fn base_url(&self) -> &str {
        "ikazuchi://local"
    }

    async fn get_popular_manga(&self, _page: u32) -> Result<MangasPage> {
        let mangas = self.scan_local_mangas(None)?;
        Ok(MangasPage {
            mangas,
            has_next_page: false,
        })
    }

    async fn get_latest_updates(&self, _page: u32) -> Result<MangasPage> {
        self.get_popular_manga(1).await
    }

    async fn get_search_manga(
        &self,
        _page: u32,
        query: &str,
        _filters: &FilterList,
    ) -> Result<MangasPage> {
        let mangas = self.scan_local_mangas(Some(query))?;
        Ok(MangasPage {
            mangas,
            has_next_page: false,
        })
    }

    async fn get_manga_details(&self, manga: &SManga) -> Result<SManga> {
        let dir = self.resolve_safe_path(&manga.url)?;
        let mut updated = manga.clone();
        updated.initialized = true;
        if updated.thumbnail_url.is_none() {
            updated.thumbnail_url = find_local_cover(&dir);
        }
        apply_dir_comic_info(&dir, &mut updated);
        Ok(updated)
    }

    async fn get_chapter_list(&self, manga: &SManga) -> Result<Vec<SChapter>> {
        let dir = self.resolve_safe_path(&manga.url)?;
        if !dir.exists() {
            return Ok(Vec::new());
        }

        let entries = std::fs::read_dir(&dir)
            .map_err(|e| IkazuchiError::Parse(format!("Failed to read local manga dir: {e}")))?;

        let mut chapters = Vec::new();
        let folder_rel = manga.url.trim_matches('/');

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
                continue;
            };
            if !ext.eq_ignore_ascii_case("cbz") && !ext.eq_ignore_ascii_case("zip") {
                continue;
            }
            let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(file_name);

            let cbz_info = read_cbz_comic_info(&path);
            let ch_name = cbz_info
                .as_ref()
                .and_then(|i| i.title.clone())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| stem.to_string());

            let ch_num = cbz_info
                .as_ref()
                .and_then(|i| i.number)
                .unwrap_or_else(|| parse_chapter_number(&ch_name, &manga.title));

            let scanlator = cbz_info
                .and_then(|i| i.translator)
                .filter(|s| !s.is_empty())
                .or_else(|| Some("Local".to_string()));

            let modified_ms = entry
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);

            chapters.push(SChapter {
                url: format!("{folder_rel}/{file_name}"),
                name: ch_name,
                chapter_number: ch_num,
                date_upload: modified_ms,
                scanlator,
            });
        }

        // Sort descending by chapter number (newest first, matching Mihon convention)
        chapters.sort_by(|a, b| b.chapter_number.total_cmp(&a.chapter_number));
        Ok(chapters)
    }

    async fn get_page_list(&self, chapter: &SChapter) -> Result<Vec<Page>> {
        let cbz_path = self.resolve_safe_path(&chapter.url)?;
        let entries = list_cbz_pages(&cbz_path)?;
        let cbz_str = cbz_path.display().to_string();

        let pages = entries
            .into_iter()
            .enumerate()
            .map(|(idx, _)| Page {
                index: idx as u32,
                url: String::new(),
                image_url: Some(format!("ikazuchi://localhost/cbz/{idx}?path={cbz_str}")),
            })
            .collect();

        Ok(pages)
    }
}
