//! # `ikazuchi-db`
//!
//! SQLite database engine (`rusqlite`) implementing Mihon's relational schema
//! (`manga`, `chapter`, `category`, `manga_category`, `history`, `extension_store`)
//! with chapter diffing (`sync_chapters`), category management, reading history,
//! and bidirectional sync state serialization for `ikazuchi-sync`.

pub mod backup;

use ikazuchi_core::{
    AppConfig, FilterList, IkazuchiError, MangaStatus, Result, SChapter, SManga, UpdateStrategy,
};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub use backup::{
    BackupCategory, BackupChapter, BackupExtensionStore, BackupHistory, BackupImportReport,
    BackupManga, BackupSource, BackupTracking, MihonBackup, decode_mihon_backup_bytes,
    encode_mihon_backup_bytes,
};

/// Stored chapter record including user reading progress and database ID.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChapterRecord {
    pub id: i64,
    pub manga_id: i64,
    pub chapter: SChapter,
    pub read: bool,
    pub bookmark: bool,
    pub last_page_read: u32,
    pub source_order: i32,
    pub sync_updated_at: i64,
}

/// User-defined library category (e.g. "Reading", "Manhwa", "Seinen").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Category {
    pub id: i64,
    pub name: String,
    pub sort_order: i32,
    pub flags: i32,
    pub sync_updated_at: i64,
}

/// Enriched library manga entry for the Sanctuary (Library) grid/list view.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LibraryMangaItem {
    pub id: i64,
    pub source_id: i64,
    pub manga: SManga,
    pub favorite_at: i64,
    pub unread_count: u32,
    pub total_chapters: u32,
    pub categories: Vec<String>,
    pub reader_flags: i32,
    pub sync_updated_at: i64,
}

/// Entry for the Storm (Recent Chapter Updates) feed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecentUpdateItem {
    pub manga_id: i64,
    pub manga_title: String,
    pub manga_cover: Option<String>,
    pub chapter: ChapterRecord,
}

/// Entry for the Storm (Reading History Timeline) feed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryItem {
    pub history_id: i64,
    pub manga_id: i64,
    pub manga_title: String,
    pub manga_cover: Option<String>,
    pub chapter_id: i64,
    pub chapter_name: String,
    pub chapter_number: f32,
    pub last_page_read: u32,
    pub read_at: i64,
    pub read_duration_sec: i64,
}

/// Configured Mihon-compatible extension store repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtensionStoreRecord {
    pub index_url: String,
    pub name: String,
    pub signing_key: String,
}

/// External tracker record (`manga_track` table: AniList, MyAnimeList, MangaUpdates, etc.).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MangaTrackRecord {
    pub id: i64,
    pub manga_id: i64,
    pub tracker_id: i64,
    pub remote_id: i64,
    pub library_id: Option<i64>,
    pub title: String,
    pub last_chapter_read: f64,
    pub total_chapters: i64,
    pub status: i64,
    pub score: f64,
    pub remote_url: String,
    pub start_date: i64,
    pub finish_date: i64,
    pub private: bool,
}

/// Summary report returned by [`Database::migrate_manga`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MigrationReport {
    pub old_manga_id: i64,
    pub new_manga_id: i64,
    pub chapters_matched: usize,
    pub categories_transferred: usize,
    pub tracks_transferred: usize,
    pub custom_info_transferred: bool,
}

/// User-defined custom metadata overrides for a manga (Yokai / J2K `custom_manga_info` table & `.tachibk` tags `602, 800..805`).
/// Overrides `title`, `author`, `artist`, `description`, `genre`, and `status` without being overwritten by source updates.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomMangaInfo {
    pub title: Option<String>,
    pub author: Option<String>,
    pub artist: Option<String>,
    pub description: Option<String>,
    pub genre: Option<Vec<String>>,
    pub status: Option<MangaStatus>,
}

impl CustomMangaInfo {
    pub fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.author.is_none()
            && self.artist.is_none()
            && self.description.is_none()
            && self.genre.is_none()
            && self.status.is_none()
    }
}

/// Saved catalog search query & filter preset per source (Yokai `saved_search` table).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedSearchRecord {
    pub id: i64,
    pub source_id: i64,
    pub name: String,
    pub query: String,
    pub filters: FilterList,
}

/// Aggregated library and reading statistics (Yokai `StatsPresenter.kt`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LibraryStats {
    pub total_library_mangas: usize,
    pub total_chapters: usize,
    pub total_chapters_read: usize,
    pub total_read_duration_sec: i64,
    pub tracked_mangas_count: usize,
    pub mean_tracker_score: f64,
}

/// Smart restrictions for filtering which library entries should be polled during a
/// Scheduled or Manual Library Update (matching Mihon's `LibraryUpdateWorker.kt`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibraryUpdateConfig {
    /// Skip mangas with `MangaStatus::Completed` (`MANGA_NON_COMPLETED`).
    pub skip_completed: bool,
    /// Skip mangas that already have `unread_count > 0` (`MANGA_HAS_UNREAD`).
    pub skip_unread: bool,
    /// Skip mangas with `total_chapters > 0` where the user hasn't read any chapter yet (`MANGA_NON_READ`).
    pub skip_unstarted: bool,
    /// Skip mangas with `UpdateStrategy::OnlyFetchOnce` when `total_chapters > 0`.
    pub respect_update_strategy: bool,
    /// If non-empty, only update mangas belonging to at least one of these category names.
    pub included_categories: Vec<String>,
    /// Never update mangas belonging to any of these category names.
    pub excluded_categories: Vec<String>,
    /// Minimum cooldown in milliseconds since the manga's last chapter check (`MANGA_OUTSIDE_RELEASE_PERIOD`).
    pub min_update_interval_ms: i64,
    /// Maximum number of distinct sources to poll in parallel (`Semaphore(5)` in Mihon).
    pub max_parallel_sources: usize,
}

impl Default for LibraryUpdateConfig {
    fn default() -> Self {
        Self {
            skip_completed: true,
            skip_unread: false,
            skip_unstarted: false,
            respect_update_strategy: true,
            included_categories: Vec::new(),
            excluded_categories: Vec::new(),
            min_update_interval_ms: 0,
            max_parallel_sources: 5,
        }
    }
}

impl LibraryUpdateConfig {
    /// Constructs a [`LibraryUpdateConfig`] from the user's persistent [`AppConfig`].
    pub fn from_app_config(cfg: &AppConfig) -> Self {
        let min_update_interval_ms = if cfg.library.skip_outside_release_period {
            i64::from(cfg.library.auto_update_interval_hours) * 3_600_000
        } else {
            0
        };
        Self {
            skip_completed: cfg.library.update_only_ongoing,
            skip_unread: cfg.library.skip_unread_entries,
            skip_unstarted: cfg.library.skip_unstarted_entries,
            respect_update_strategy: true,
            included_categories: cfg.library.included_categories.clone(),
            excluded_categories: cfg.library.excluded_categories.clone(),
            min_update_interval_ms,
            max_parallel_sources: cfg.downloads.parallel_source_limit.clamp(1, 10),
        }
    }
}

/// Reason why a library manga was skipped during a Scheduled or Manual Library Update.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum UpdateSkipReason {
    OnlyFetchOnce,
    Completed,
    HasUnread,
    NotStarted,
    ExcludedCategory,
    NotInIncludedCategory,
    OutsideReleasePeriod,
}

pub struct Database {
    conn: Connection,
}

impl Database {
    /// Opens or creates the Ikazuchi SQLite database at the given path and runs migrations.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let conn = Connection::open(path).map_err(|e| IkazuchiError::Database(e.to_string()))?;
        let db = Self { conn };
        db.init_schema()?;
        Ok(db)
    }

    /// Opens an in-memory database (useful for tests and fast prototyping).
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory().map_err(|e| IkazuchiError::Database(e.to_string()))?;
        let db = Self { conn };
        db.init_schema()?;
        Ok(db)
    }

    fn init_schema(&self) -> Result<()> {
        self.conn
            .execute_batch(
                "
                PRAGMA foreign_keys = ON;
                PRAGMA journal_mode = WAL;

                CREATE TABLE IF NOT EXISTS manga (
                    id INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,
                    source_id INTEGER NOT NULL,
                    remote_url TEXT NOT NULL,
                    remote_title TEXT NOT NULL,
                    remote_author TEXT,
                    remote_artist TEXT,
                    remote_description TEXT,
                    remote_genre TEXT NOT NULL DEFAULT '[]',
                    remote_status INTEGER NOT NULL DEFAULT 0,
                    remote_cover TEXT,
                    remote_update_strategy INTEGER NOT NULL DEFAULT 0,
                    user_favorite_at INTEGER,
                    user_notes TEXT NOT NULL DEFAULT '',
                    user_reader_flags INTEGER NOT NULL DEFAULT 0,
                    user_chapter_flags INTEGER NOT NULL DEFAULT 0,
                    state_chapter_last_update INTEGER,
                    state_initialized INTEGER NOT NULL DEFAULT 0,
                    sync_updated_at INTEGER NOT NULL DEFAULT 0,
                    UNIQUE(source_id, remote_url)
                );

                CREATE INDEX IF NOT EXISTS idx_manga_favorite
                    ON manga(user_favorite_at) WHERE user_favorite_at IS NOT NULL;

                CREATE TABLE IF NOT EXISTS chapter (
                    id INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,
                    manga_id INTEGER NOT NULL,
                    remote_url TEXT NOT NULL,
                    remote_name TEXT NOT NULL,
                    remote_scanlator TEXT,
                    remote_chapter_number REAL NOT NULL DEFAULT -1.0,
                    remote_date_upload INTEGER NOT NULL DEFAULT 0,
                    remote_order INTEGER NOT NULL DEFAULT 0,
                    user_read INTEGER NOT NULL DEFAULT 0,
                    user_bookmark INTEGER NOT NULL DEFAULT 0,
                    user_last_page_read INTEGER NOT NULL DEFAULT 0,
                    state_date_fetch INTEGER NOT NULL DEFAULT 0,
                    sync_updated_at INTEGER NOT NULL DEFAULT 0,
                    UNIQUE(manga_id, remote_url),
                    FOREIGN KEY(manga_id) REFERENCES manga(id) ON DELETE CASCADE
                );

                CREATE INDEX IF NOT EXISTS idx_chapter_date_fetch
                    ON chapter(state_date_fetch DESC);

                CREATE TABLE IF NOT EXISTS category (
                    id INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,
                    name TEXT NOT NULL UNIQUE,
                    sort_order INTEGER NOT NULL,
                    flags INTEGER NOT NULL DEFAULT 0,
                    sync_updated_at INTEGER NOT NULL DEFAULT 0
                );

                CREATE TABLE IF NOT EXISTS manga_category (
                    manga_id INTEGER NOT NULL,
                    category_id INTEGER NOT NULL,
                    PRIMARY KEY(manga_id, category_id),
                    FOREIGN KEY(manga_id) REFERENCES manga(id) ON DELETE CASCADE,
                    FOREIGN KEY(category_id) REFERENCES category(id) ON DELETE CASCADE
                );

                CREATE TABLE IF NOT EXISTS history (
                    id INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,
                    manga_id INTEGER NOT NULL,
                    chapter_id INTEGER NOT NULL UNIQUE,
                    read_at INTEGER NOT NULL,
                    read_duration_sec INTEGER NOT NULL DEFAULT 0,
                    FOREIGN KEY(manga_id) REFERENCES manga(id) ON DELETE CASCADE,
                    FOREIGN KEY(chapter_id) REFERENCES chapter(id) ON DELETE CASCADE
                );

                CREATE INDEX IF NOT EXISTS idx_history_read_at
                    ON history(read_at DESC);

                CREATE TABLE IF NOT EXISTS extension_store (
                    index_url TEXT NOT NULL PRIMARY KEY,
                    name TEXT NOT NULL,
                    signing_key TEXT NOT NULL DEFAULT ''
                );

                CREATE TABLE IF NOT EXISTS manga_track (
                    id INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,
                    manga_id INTEGER NOT NULL,
                    tracker_id INTEGER NOT NULL,
                    remote_id INTEGER NOT NULL,
                    library_id INTEGER,
                    title TEXT NOT NULL DEFAULT '',
                    last_chapter_read REAL NOT NULL DEFAULT 0.0,
                    total_chapters INTEGER NOT NULL DEFAULT 0,
                    status INTEGER NOT NULL DEFAULT 0,
                    score REAL NOT NULL DEFAULT 0.0,
                    remote_url TEXT NOT NULL DEFAULT '',
                    start_date INTEGER NOT NULL DEFAULT 0,
                    finish_date INTEGER NOT NULL DEFAULT 0,
                    private INTEGER NOT NULL DEFAULT 0,
                    UNIQUE(manga_id, tracker_id),
                    FOREIGN KEY(manga_id) REFERENCES manga(id) ON DELETE CASCADE
                );

                CREATE TABLE IF NOT EXISTS excluded_scanlator (
                    manga_id INTEGER NOT NULL,
                    scanlator TEXT NOT NULL,
                    PRIMARY KEY(manga_id, scanlator),
                    FOREIGN KEY(manga_id) REFERENCES manga(id) ON DELETE CASCADE
                );

                CREATE TABLE IF NOT EXISTS custom_manga_info (
                    manga_id INTEGER NOT NULL PRIMARY KEY,
                    custom_title TEXT,
                    custom_author TEXT,
                    custom_artist TEXT,
                    custom_description TEXT,
                    custom_genre TEXT,
                    custom_status INTEGER,
                    FOREIGN KEY(manga_id) REFERENCES manga(id) ON DELETE CASCADE
                );

                CREATE TABLE IF NOT EXISTS saved_search (
                    id INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,
                    source_id INTEGER NOT NULL,
                    name TEXT NOT NULL,
                    query TEXT NOT NULL DEFAULT '',
                    filters_json TEXT NOT NULL DEFAULT '[]',
                    UNIQUE(source_id, name)
                );
                ",
            )
            .map_err(|e| IkazuchiError::Database(e.to_string()))?;
        Ok(())
    }

    // =========================================================================
    // MANGA & LIBRARY CRUD
    // =========================================================================

    /// Upserts a discovered or library manga entry and returns its database row ID.
    pub fn upsert_manga(&self, source_id: i64, manga: &SManga, favorite_at: Option<i64>, now_ms: i64) -> Result<i64> {
        let genre_json = serde_json::to_string(&manga.genre).unwrap_or_else(|_| "[]".to_string());
        let strategy_int = match manga.update_strategy {
            UpdateStrategy::AlwaysUpdate => 0,
            UpdateStrategy::OnlyFetchOnce => 1,
        };
        self.conn
            .execute(
                "INSERT INTO manga (
                    source_id, remote_url, remote_title, remote_author, remote_artist,
                    remote_description, remote_genre, remote_status, remote_cover,
                    remote_update_strategy, user_favorite_at, state_initialized, sync_updated_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
                ON CONFLICT(source_id, remote_url) DO UPDATE SET
                    remote_title = excluded.remote_title,
                    remote_author = COALESCE(excluded.remote_author, remote_author),
                    remote_artist = COALESCE(excluded.remote_artist, remote_artist),
                    remote_description = COALESCE(excluded.remote_description, remote_description),
                    remote_genre = CASE
                        WHEN excluded.remote_genre != '[]' THEN excluded.remote_genre
                        ELSE remote_genre
                    END,
                    remote_status = CASE
                        WHEN excluded.remote_status != 0 THEN excluded.remote_status
                        ELSE remote_status
                    END,
                    remote_cover = COALESCE(excluded.remote_cover, remote_cover),
                    remote_update_strategy = excluded.remote_update_strategy,
                    user_favorite_at = COALESCE(excluded.user_favorite_at, user_favorite_at),
                    state_initialized = MAX(state_initialized, excluded.state_initialized),
                    sync_updated_at = MAX(sync_updated_at, excluded.sync_updated_at)
                ",
                params![
                    source_id,
                    manga.url,
                    manga.title,
                    manga.author,
                    manga.artist,
                    manga.description,
                    genre_json,
                    manga.status as i32,
                    manga.thumbnail_url,
                    strategy_int,
                    favorite_at,
                    i32::from(manga.initialized),
                    now_ms,
                ],
            )
            .map_err(|e| IkazuchiError::Database(e.to_string()))?;

        let id: i64 = self
            .conn
            .query_row(
                "SELECT id FROM manga WHERE source_id = ?1 AND remote_url = ?2",
                params![source_id, manga.url],
                |row| row.get(0),
            )
            .map_err(|e| IkazuchiError::Database(e.to_string()))?;
        Ok(id)
    }

    /// Toggles or sets whether a manga is in the user's Sanctuary (Library).
    pub fn set_manga_favorite(&self, manga_id: i64, favorite_at: Option<i64>, now_ms: i64) -> Result<()> {
        self.conn
            .execute(
                "UPDATE manga SET user_favorite_at = ?1, sync_updated_at = ?2 WHERE id = ?3",
                params![favorite_at, now_ms, manga_id],
            )
            .map_err(|e| IkazuchiError::Database(e.to_string()))?;
        Ok(())
    }

    /// Updates per-manga reader mode/orientation bitmask flags.
    pub fn set_manga_reader_flags(&self, manga_id: i64, reader_flags: i32, now_ms: i64) -> Result<()> {
        self.conn
            .execute(
                "UPDATE manga SET user_reader_flags = ?1, sync_updated_at = ?2 WHERE id = ?3",
                params![reader_flags, now_ms, manga_id],
            )
            .map_err(|e| IkazuchiError::Database(e.to_string()))?;
        Ok(())
    }

    /// Retrieves a single manga by its primary key ID, applying any user `custom_manga_info` overrides.
    /// Safeguard #8: `unread_count` and `total_chapters` automatically exclude any scanlator
    /// present in `excluded_scanlator` for this manga.
    pub fn get_manga_by_id(&self, manga_id: i64) -> Result<Option<LibraryMangaItem>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT M.id, M.source_id, M.remote_url,
                        COALESCE(CMI.custom_title, M.remote_title),
                        COALESCE(CMI.custom_author, M.remote_author),
                        COALESCE(CMI.custom_artist, M.remote_artist),
                        COALESCE(CMI.custom_description, M.remote_description),
                        COALESCE(CMI.custom_genre, M.remote_genre),
                        COALESCE(CMI.custom_status, M.remote_status),
                        M.remote_cover,
                        M.state_initialized, COALESCE(M.user_favorite_at, 0), M.user_reader_flags, M.sync_updated_at,
                        (SELECT COUNT(*) FROM chapter C WHERE C.manga_id = M.id AND C.user_read = 0
                         AND COALESCE(C.remote_scanlator, '') NOT IN (SELECT scanlator FROM excluded_scanlator E WHERE E.manga_id = M.id)) AS unread_count,
                        (SELECT COUNT(*) FROM chapter C WHERE C.manga_id = M.id
                         AND COALESCE(C.remote_scanlator, '') NOT IN (SELECT scanlator FROM excluded_scanlator E WHERE E.manga_id = M.id)) AS total_chapters,
                        M.remote_update_strategy
                 FROM manga M
                 LEFT JOIN custom_manga_info CMI ON CMI.manga_id = M.id
                 WHERE M.id = ?1",
            )
            .map_err(|e| IkazuchiError::Database(e.to_string()))?;

        let item_opt = stmt
            .query_row(params![manga_id], map_library_manga_row)
            .optional()
            .map_err(|e| IkazuchiError::Database(e.to_string()))?;

        if let Some(mut item) = item_opt {
            item.categories = self
                .get_categories_for_manga(item.id)?
                .into_iter()
                .map(|c| c.name)
                .collect();
            Ok(Some(item))
        } else {
            Ok(None)
        }
    }

    /// Retrieves a single manga by its `(source_id, remote_url)` unique key.
    pub fn get_manga_by_source_and_url(
        &self,
        source_id: i64,
        remote_url: &str,
    ) -> Result<Option<LibraryMangaItem>> {
        let id_opt: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM manga WHERE source_id = ?1 AND remote_url = ?2",
                params![source_id, remote_url],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| IkazuchiError::Database(e.to_string()))?;

        match id_opt {
            Some(id) => self.get_manga_by_id(id),
            None => Ok(None),
        }
    }

    /// Lists all favorited (library) mangas with unread counts, total chapters, category names,
    /// and any user `custom_manga_info` overrides applied.
    /// Safeguard #8: `unread_count` and `total_chapters` automatically exclude any scanlator
    /// present in `excluded_scanlator` for each manga.
    pub fn get_library_manga(&self) -> Result<Vec<LibraryMangaItem>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT M.id, M.source_id, M.remote_url,
                        COALESCE(CMI.custom_title, M.remote_title),
                        COALESCE(CMI.custom_author, M.remote_author),
                        COALESCE(CMI.custom_artist, M.remote_artist),
                        COALESCE(CMI.custom_description, M.remote_description),
                        COALESCE(CMI.custom_genre, M.remote_genre),
                        COALESCE(CMI.custom_status, M.remote_status),
                        M.remote_cover,
                        M.state_initialized, M.user_favorite_at, M.user_reader_flags, M.sync_updated_at,
                        (SELECT COUNT(*) FROM chapter C WHERE C.manga_id = M.id AND C.user_read = 0
                         AND COALESCE(C.remote_scanlator, '') NOT IN (SELECT scanlator FROM excluded_scanlator E WHERE E.manga_id = M.id)) AS unread_count,
                        (SELECT COUNT(*) FROM chapter C WHERE C.manga_id = M.id
                         AND COALESCE(C.remote_scanlator, '') NOT IN (SELECT scanlator FROM excluded_scanlator E WHERE E.manga_id = M.id)) AS total_chapters,
                        M.remote_update_strategy
                 FROM manga M
                 LEFT JOIN custom_manga_info CMI ON CMI.manga_id = M.id
                 WHERE M.user_favorite_at IS NOT NULL
                 ORDER BY M.user_favorite_at DESC",
            )
            .map_err(|e| IkazuchiError::Database(e.to_string()))?;

        let rows = stmt
            .query_map([], map_library_manga_row)
            .map_err(|e| IkazuchiError::Database(e.to_string()))?;

        let mut items = Vec::new();
        for r in rows {
            let mut item = r.map_err(|e| IkazuchiError::Database(e.to_string()))?;
            item.categories = self
                .get_categories_for_manga(item.id)?
                .into_iter()
                .map(|c| c.name)
                .collect();
            items.push(item);
        }
        Ok(items)
    }

    /// Evaluates Mihon's smart library update restrictions (`LibraryUpdateWorker.kt`) and returns
    /// `(eligible_mangas, skipped_mangas_with_reason)`.
    ///
    /// If `target_category` is `Some(cat)`, only mangas in that specific category are evaluated
    /// (bypassing global category include/exclude filters, matching Mihon's manual category update).
    pub fn filter_library_for_update(
        &self,
        config: &LibraryUpdateConfig,
        target_category: Option<&str>,
        now_ms: i64,
    ) -> Result<(Vec<LibraryMangaItem>, Vec<(i64, String, UpdateSkipReason)>)> {
        let library = self.get_library_manga()?;
        let mut eligible = Vec::new();
        let mut skipped = Vec::new();

        for item in library {
            if let Some(target_cat) = target_category {
                if !item.categories.iter().any(|c| c == target_cat) {
                    continue;
                }
            } else {
                if !config.excluded_categories.is_empty()
                    && item
                        .categories
                        .iter()
                        .any(|c| config.excluded_categories.contains(c))
                {
                    skipped.push((item.id, item.manga.title, UpdateSkipReason::ExcludedCategory));
                    continue;
                }
                if !config.included_categories.is_empty()
                    && !item
                        .categories
                        .iter()
                        .any(|c| config.included_categories.contains(c))
                {
                    skipped.push((
                        item.id,
                        item.manga.title,
                        UpdateSkipReason::NotInIncludedCategory,
                    ));
                    continue;
                }
            }

            if config.respect_update_strategy
                && item.manga.update_strategy == UpdateStrategy::OnlyFetchOnce
                && item.total_chapters > 0
            {
                skipped.push((item.id, item.manga.title, UpdateSkipReason::OnlyFetchOnce));
                continue;
            }

            if config.skip_completed && item.manga.status == MangaStatus::Completed {
                skipped.push((item.id, item.manga.title, UpdateSkipReason::Completed));
                continue;
            }

            if config.skip_unread && item.unread_count > 0 {
                skipped.push((item.id, item.manga.title, UpdateSkipReason::HasUnread));
                continue;
            }

            let read_count = item.total_chapters.saturating_sub(item.unread_count);
            if config.skip_unstarted && item.total_chapters > 0 && read_count == 0 {
                skipped.push((item.id, item.manga.title, UpdateSkipReason::NotStarted));
                continue;
            }

            if config.min_update_interval_ms > 0 {
                let last_update: Option<i64> = self
                    .conn
                    .query_row(
                        "SELECT state_chapter_last_update FROM manga WHERE id = ?1",
                        params![item.id],
                        |r| r.get(0),
                    )
                    .optional()
                    .map_err(|e| IkazuchiError::Database(e.to_string()))?
                    .flatten();

                if let Some(last_ts) = last_update {
                    if now_ms.saturating_sub(last_ts) < config.min_update_interval_ms {
                        skipped.push((
                            item.id,
                            item.manga.title,
                            UpdateSkipReason::OutsideReleasePeriod,
                        ));
                        continue;
                    }
                }
            }

            eligible.push(item);
        }

        eligible.sort_by(|a, b| a.manga.title.cmp(&b.manga.title));
        Ok((eligible, skipped))
    }

    // =========================================================================
    // CHAPTER SYNC & READING PROGRESS
    // =========================================================================

    /// Syncs remote chapters for `manga_id`, updates `state_chapter_last_update`, and returns
    /// only the **newly discovered visible chapters** (excluding any scanlator in `excluded_scanlator`).
    pub fn sync_chapters_detecting_new(
        &self,
        manga_id: i64,
        remote_chapters: &[SChapter],
        now_ms: i64,
    ) -> Result<Vec<ChapterRecord>> {
        let existing_urls: std::collections::HashSet<String> = self
            .get_chapters_by_manga_id(manga_id)?
            .into_iter()
            .map(|c| c.chapter.url)
            .collect();

        self.sync_chapters(manga_id, remote_chapters, now_ms)?;
        self.conn
            .execute(
                "UPDATE manga SET state_chapter_last_update = ?1, sync_updated_at = MAX(sync_updated_at, ?1) WHERE id = ?2",
                params![now_ms, manga_id],
            )
            .map_err(|e| IkazuchiError::Database(e.to_string()))?;

        let visible_after = self.get_visible_chapters_by_manga_id(manga_id)?;
        let newly_added: Vec<ChapterRecord> = visible_after
            .into_iter()
            .filter(|c| !existing_urls.contains(&c.chapter.url))
            .collect();

        Ok(newly_added)
    }

    /// Syncs remote chapters from a `MangaSource` into the database for `manga_id`,
    /// preserving `user_read`, `user_bookmark`, and `user_last_page_read` on existing chapters.
    pub fn sync_chapters(&self, manga_id: i64, remote_chapters: &[SChapter], now_ms: i64) -> Result<usize> {
        let mut inserted_or_updated = 0;
        for (idx, ch) in remote_chapters.iter().enumerate() {
            let affected = self
                .conn
                .execute(
                    "INSERT INTO chapter (
                        manga_id, remote_url, remote_name, remote_scanlator,
                        remote_chapter_number, remote_date_upload, remote_order,
                        user_read, user_bookmark, user_last_page_read,
                        state_date_fetch, sync_updated_at
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0, 0, 0, ?8, ?8)
                    ON CONFLICT(manga_id, remote_url) DO UPDATE SET
                        remote_name = excluded.remote_name,
                        remote_scanlator = COALESCE(excluded.remote_scanlator, remote_scanlator),
                        remote_chapter_number = excluded.remote_chapter_number,
                        remote_date_upload = CASE
                            WHEN excluded.remote_date_upload > 0 THEN excluded.remote_date_upload
                            ELSE remote_date_upload
                        END,
                        remote_order = excluded.remote_order
                    ",
                    params![
                        manga_id,
                        ch.url,
                        ch.name,
                        ch.scanlator,
                        f64::from(ch.chapter_number),
                        ch.date_upload,
                        idx as i32,
                        now_ms,
                    ],
                )
                .map_err(|e| IkazuchiError::Database(e.to_string()))?;
            inserted_or_updated += affected;
        }
        Ok(inserted_or_updated)
    }

    /// Updates reading progress for a chapter and records an entry in the `history` table.
    pub fn update_chapter_progress(
        &self,
        chapter_id: i64,
        read: bool,
        bookmark: bool,
        last_page_read: u32,
        now_ms: i64,
    ) -> Result<()> {
        self.conn
            .execute(
                "UPDATE chapter
                 SET user_read = ?1,
                     user_bookmark = ?2,
                     user_last_page_read = ?3,
                     sync_updated_at = ?4
                 WHERE id = ?5",
                params![
                    i32::from(read),
                    i32::from(bookmark),
                    last_page_read,
                    now_ms,
                    chapter_id
                ],
            )
            .map_err(|e| IkazuchiError::Database(e.to_string()))?;

        self.conn
            .execute(
                "UPDATE manga SET sync_updated_at = MAX(sync_updated_at, ?1)
                 WHERE id = (SELECT manga_id FROM chapter WHERE id = ?2)",
                params![now_ms, chapter_id],
            )
            .map_err(|e| IkazuchiError::Database(e.to_string()))?;

        self.conn
            .execute(
                "INSERT INTO history (manga_id, chapter_id, read_at, read_duration_sec)
                 SELECT manga_id, id, ?1, 0 FROM chapter WHERE id = ?2
                 ON CONFLICT(chapter_id) DO UPDATE SET read_at = excluded.read_at",
                params![now_ms, chapter_id],
            )
            .map_err(|e| IkazuchiError::Database(e.to_string()))?;
        Ok(())
    }

    /// Retrieves all chapters for a manga ordered by source order.
    pub fn get_chapters_by_manga_id(&self, manga_id: i64) -> Result<Vec<ChapterRecord>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, manga_id, remote_url, remote_name, remote_scanlator,
                        remote_chapter_number, remote_date_upload, remote_order,
                        user_read, user_bookmark, user_last_page_read, sync_updated_at
                 FROM chapter
                 WHERE manga_id = ?1
                 ORDER BY remote_order ASC",
            )
            .map_err(|e| IkazuchiError::Database(e.to_string()))?;

        let rows = stmt
            .query_map(params![manga_id], |row| map_chapter_row(row, 0))
            .map_err(|e| IkazuchiError::Database(e.to_string()))?;

        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| IkazuchiError::Database(e.to_string()))
    }

    // =========================================================================
    // CATEGORIES
    // =========================================================================

    /// Creates a new category (or returns the existing one if `name` already exists).
    pub fn create_category(&self, name: &str, flags: i32, now_ms: i64) -> Result<i64> {
        let trimmed = name.trim();
        self.conn
            .execute(
                "INSERT INTO category (name, sort_order, flags, sync_updated_at)
                 VALUES (?1, (SELECT COALESCE(MAX(sort_order), -1) + 1 FROM category), ?2, ?3)
                 ON CONFLICT(name) DO UPDATE SET sync_updated_at = excluded.sync_updated_at",
                params![trimmed, flags, now_ms],
            )
            .map_err(|e| IkazuchiError::Database(e.to_string()))?;

        let id: i64 = self
            .conn
            .query_row("SELECT id FROM category WHERE name = ?1", params![trimmed], |r| r.get(0))
            .map_err(|e| IkazuchiError::Database(e.to_string()))?;
        Ok(id)
    }

    /// Lists all user categories ordered by `sort_order`.
    pub fn get_categories(&self) -> Result<Vec<Category>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, name, sort_order, flags, sync_updated_at FROM category ORDER BY sort_order ASC")
            .map_err(|e| IkazuchiError::Database(e.to_string()))?;

        stmt.query_map([], map_category_row)
            .map_err(|e| IkazuchiError::Database(e.to_string()))?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| IkazuchiError::Database(e.to_string()))
    }

    /// Replaces the categories assigned to `manga_id` using category names (auto-creating missing categories).
    pub fn set_manga_categories_by_name(&self, manga_id: i64, category_names: &[String], now_ms: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM manga_category WHERE manga_id = ?1", params![manga_id])
            .map_err(|e| IkazuchiError::Database(e.to_string()))?;

        for name in category_names {
            if name.trim().is_empty() {
                continue;
            }
            let cat_id = self.create_category(name, 0, now_ms)?;
            self.conn
                .execute(
                    "INSERT OR IGNORE INTO manga_category (manga_id, category_id) VALUES (?1, ?2)",
                    params![manga_id, cat_id],
                )
                .map_err(|e| IkazuchiError::Database(e.to_string()))?;
        }

        self.conn
            .execute(
                "UPDATE manga SET sync_updated_at = MAX(sync_updated_at, ?1) WHERE id = ?2",
                params![now_ms, manga_id],
            )
            .map_err(|e| IkazuchiError::Database(e.to_string()))?;
        Ok(())
    }

    /// Returns all categories assigned to a specific manga.
    pub fn get_categories_for_manga(&self, manga_id: i64) -> Result<Vec<Category>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT C.id, C.name, C.sort_order, C.flags, C.sync_updated_at
                 FROM category C
                 JOIN manga_category MC ON C.id = MC.category_id
                 WHERE MC.manga_id = ?1
                 ORDER BY C.sort_order ASC",
            )
            .map_err(|e| IkazuchiError::Database(e.to_string()))?;

        stmt.query_map(params![manga_id], map_category_row)
            .map_err(|e| IkazuchiError::Database(e.to_string()))?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| IkazuchiError::Database(e.to_string()))
    }

    // =========================================================================
    // UPDATES & HISTORY (STORM REALM)
    // =========================================================================

    /// Returns the most recently fetched visible chapters across all library mangas
    /// (excluding any scanlator in `excluded_scanlator`).
    pub fn get_recent_updates(&self, limit: u32) -> Result<Vec<RecentUpdateItem>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT M.id, M.remote_title, M.remote_cover,
                        C.id, C.manga_id, C.remote_url, C.remote_name, C.remote_scanlator,
                        C.remote_chapter_number, C.remote_date_upload, C.remote_order,
                        C.user_read, C.user_bookmark, C.user_last_page_read, C.sync_updated_at
                 FROM chapter C
                 JOIN manga M ON M.id = C.manga_id
                 WHERE M.user_favorite_at IS NOT NULL
                   AND COALESCE(C.remote_scanlator, '') NOT IN (
                       SELECT scanlator FROM excluded_scanlator E WHERE E.manga_id = M.id
                   )
                 ORDER BY C.state_date_fetch DESC, C.remote_order ASC
                 LIMIT ?1",
            )
            .map_err(|e| IkazuchiError::Database(e.to_string()))?;

        stmt.query_map(params![limit], |row| {
            Ok(RecentUpdateItem {
                manga_id: row.get(0)?,
                manga_title: row.get(1)?,
                manga_cover: row.get(2)?,
                chapter: map_chapter_row(row, 3)?,
            })
        })
        .map_err(|e| IkazuchiError::Database(e.to_string()))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| IkazuchiError::Database(e.to_string()))
    }

    /// Returns the most recently read chapters from the `history` table.
    pub fn get_recent_history(&self, limit: u32) -> Result<Vec<HistoryItem>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT H.id, M.id, M.remote_title, M.remote_cover,
                        C.id, C.remote_name, C.remote_chapter_number, C.user_last_page_read,
                        H.read_at, H.read_duration_sec
                 FROM history H
                 JOIN manga M ON M.id = H.manga_id
                 JOIN chapter C ON C.id = H.chapter_id
                 ORDER BY H.read_at DESC
                 LIMIT ?1",
            )
            .map_err(|e| IkazuchiError::Database(e.to_string()))?;

        let rows = stmt
            .query_map(params![limit], |row| {
                let ch_num: f64 = row.get(6)?;
                Ok(HistoryItem {
                    history_id: row.get(0)?,
                    manga_id: row.get(1)?,
                    manga_title: row.get(2)?,
                    manga_cover: row.get(3)?,
                    chapter_id: row.get(4)?,
                    chapter_name: row.get(5)?,
                    chapter_number: ch_num as f32,
                    last_page_read: row.get(7)?,
                    read_at: row.get(8)?,
                    read_duration_sec: row.get(9)?,
                })
            })
            .map_err(|e| IkazuchiError::Database(e.to_string()))?;

        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| IkazuchiError::Database(e.to_string()))?);
        }
        Ok(out)
    }

    // =========================================================================
    // EXTENSION STORE REPOSITORIES
    // =========================================================================

    pub fn upsert_extension_store(&self, index_url: &str, name: &str, signing_key: &str) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO extension_store (index_url, name, signing_key)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(index_url) DO UPDATE SET name = excluded.name, signing_key = excluded.signing_key",
                params![index_url.trim(), name.trim(), signing_key.trim()],
            )
            .map_err(|e| IkazuchiError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn get_extension_stores(&self) -> Result<Vec<ExtensionStoreRecord>> {
        let mut stmt = self
            .conn
            .prepare("SELECT index_url, name, signing_key FROM extension_store ORDER BY name ASC")
            .map_err(|e| IkazuchiError::Database(e.to_string()))?;
        let rows = stmt
            .query_map([], |row| {
                Ok(ExtensionStoreRecord {
                    index_url: row.get(0)?,
                    name: row.get(1)?,
                    signing_key: row.get(2)?,
                })
            })
            .map_err(|e| IkazuchiError::Database(e.to_string()))?;

        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| IkazuchiError::Database(e.to_string()))?);
        }
        Ok(out)
    }

    // =========================================================================
    // HISTORY UPSERT, EXCLUDED SCANLATORS, TRACKERS & SOURCE MIGRATION
    // =========================================================================

    /// Upserts a reading history entry for `chapter_id`, keeping the latest `read_at` timestamp
    /// and accumulating `read_duration_sec`.
    pub fn upsert_history_entry(
        &self,
        chapter_id: i64,
        read_at: i64,
        read_duration_sec: i64,
    ) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO history (manga_id, chapter_id, read_at, read_duration_sec)
                 SELECT manga_id, id, ?2, ?3 FROM chapter WHERE id = ?1
                 ON CONFLICT(chapter_id) DO UPDATE SET
                     read_at = MAX(history.read_at, excluded.read_at),
                     read_duration_sec = history.read_duration_sec + excluded.read_duration_sec",
                params![chapter_id, read_at, read_duration_sec],
            )
            .map_err(|e| IkazuchiError::Database(e.to_string()))?;
        Ok(())
    }

    /// Excludes a scanlator group for `manga_id`.
    /// Excluded scanlators are hidden from [`Database::get_visible_chapters_by_manga_id`] and
    /// excluded from `unread_count` in [`Database::get_library_manga`].
    pub fn add_excluded_scanlator(&self, manga_id: i64, scanlator: &str) -> Result<()> {
        let trimmed = scanlator.trim();
        if trimmed.is_empty() {
            return Ok(());
        }
        self.conn
            .execute(
                "INSERT OR IGNORE INTO excluded_scanlator (manga_id, scanlator) VALUES (?1, ?2)",
                params![manga_id, trimmed],
            )
            .map_err(|e| IkazuchiError::Database(e.to_string()))?;
        Ok(())
    }

    /// Removes a scanlator group from the exclusion list for `manga_id`.
    pub fn remove_excluded_scanlator(&self, manga_id: i64, scanlator: &str) -> Result<()> {
        self.conn
            .execute(
                "DELETE FROM excluded_scanlator WHERE manga_id = ?1 AND scanlator = ?2",
                params![manga_id, scanlator.trim()],
            )
            .map_err(|e| IkazuchiError::Database(e.to_string()))?;
        Ok(())
    }

    /// Lists all excluded scanlators for `manga_id`.
    pub fn get_excluded_scanlators(&self, manga_id: i64) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT scanlator FROM excluded_scanlator WHERE manga_id = ?1 ORDER BY scanlator ASC")
            .map_err(|e| IkazuchiError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![manga_id], |r| r.get::<_, String>(0))
            .map_err(|e| IkazuchiError::Database(e.to_string()))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| IkazuchiError::Database(e.to_string()))?);
        }
        Ok(out)
    }

    /// Retrieves visible chapters for `manga_id`, filtering out any chapter whose scanlator
    /// is in `excluded_scanlator`.
    pub fn get_visible_chapters_by_manga_id(&self, manga_id: i64) -> Result<Vec<ChapterRecord>> {
        let excluded = self.get_excluded_scanlators(manga_id)?;
        let all = self.get_chapters_by_manga_id(manga_id)?;
        if excluded.is_empty() {
            return Ok(all);
        }
        let ex_set: std::collections::HashSet<&str> = excluded.iter().map(|s| s.as_str()).collect();
        Ok(all
            .into_iter()
            .filter(|c| {
                let sc = c.chapter.scanlator.as_deref().unwrap_or("");
                !ex_set.contains(sc)
            })
            .collect())
    }

    /// Upserts an external tracker record (`manga_track`: AniList, MyAnimeList, MangaUpdates, etc.).
    pub fn upsert_manga_track(&self, track: &MangaTrackRecord) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO manga_track (
                    manga_id, tracker_id, remote_id, library_id, title,
                    last_chapter_read, total_chapters, status, score,
                    remote_url, start_date, finish_date, private
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
                 ON CONFLICT(manga_id, tracker_id) DO UPDATE SET
                    remote_id = excluded.remote_id,
                    library_id = excluded.library_id,
                    title = excluded.title,
                    last_chapter_read = MAX(manga_track.last_chapter_read, excluded.last_chapter_read),
                    total_chapters = MAX(manga_track.total_chapters, excluded.total_chapters),
                    status = excluded.status,
                    score = excluded.score,
                    remote_url = excluded.remote_url,
                    start_date = CASE WHEN manga_track.start_date > 0 THEN manga_track.start_date ELSE excluded.start_date END,
                    finish_date = MAX(manga_track.finish_date, excluded.finish_date),
                    private = excluded.private",
                params![
                    track.manga_id,
                    track.tracker_id,
                    track.remote_id,
                    track.library_id,
                    track.title,
                    track.last_chapter_read,
                    track.total_chapters,
                    track.status,
                    track.score,
                    track.remote_url,
                    track.start_date,
                    track.finish_date,
                    i32::from(track.private),
                ],
            )
            .map_err(|e| IkazuchiError::Database(e.to_string()))?;
        Ok(())
    }

    /// Lists all external tracker records linked to `manga_id`.
    pub fn get_tracks_for_manga(&self, manga_id: i64) -> Result<Vec<MangaTrackRecord>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, manga_id, tracker_id, remote_id, library_id, title,
                        last_chapter_read, total_chapters, status, score,
                        remote_url, start_date, finish_date, private
                 FROM manga_track
                 WHERE manga_id = ?1
                 ORDER BY tracker_id ASC",
            )
            .map_err(|e| IkazuchiError::Database(e.to_string()))?;

        let rows = stmt
            .query_map(params![manga_id], |row| {
                Ok(MangaTrackRecord {
                    id: row.get(0)?,
                    manga_id: row.get(1)?,
                    tracker_id: row.get(2)?,
                    remote_id: row.get(3)?,
                    library_id: row.get(4)?,
                    title: row.get(5)?,
                    last_chapter_read: row.get(6)?,
                    total_chapters: row.get(7)?,
                    status: row.get(8)?,
                    score: row.get(9)?,
                    remote_url: row.get(10)?,
                    start_date: row.get(11)?,
                    finish_date: row.get(12)?,
                    private: row.get::<_, i32>(13)? != 0,
                })
            })
            .map_err(|e| IkazuchiError::Database(e.to_string()))?;

        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| IkazuchiError::Database(e.to_string()))?);
        }
        Ok(out)
    }

    /// Migrates library state (reading progress, bookmarks, categories, reader flags, and external trackers)
    /// from `old_manga_id` to `(new_source_id, new_manga, new_chapters)`.
    ///
    /// Safeguards (#7):
    /// - If `old_manga_id == new_manga_id` (self-migration), syncs `new_chapters` and returns immediately
    ///   without unfavoriting the entry!
    /// - Numbered chapters (`chapter_number >= 0.0`) match across sources within `1e-3` epsilon (even if
    ///   the new source has multiple scanlators for the same chapter number).
    /// - Unnumbered specials (`chapter_number < 0.0`) only match by exact case-insensitive chapter title,
    ///   preventing all `-1.0` chapters from falsely inheriting read state.
    pub fn migrate_manga(
        &self,
        old_manga_id: i64,
        new_source_id: i64,
        new_manga: &SManga,
        new_chapters: &[SChapter],
        unfavorite_old: bool,
        now_ms: i64,
    ) -> Result<MigrationReport> {
        let old_entry = self.get_manga_by_id(old_manga_id)?.ok_or_else(|| {
            IkazuchiError::Database(format!("Source manga id {old_manga_id} not found for migration"))
        })?;

        let fav_ts = if old_entry.favorite_at > 0 {
            Some(old_entry.favorite_at)
        } else {
            Some(now_ms)
        };

        let new_manga_id = self.upsert_manga(new_source_id, new_manga, fav_ts, now_ms)?;
        self.sync_chapters(new_manga_id, new_chapters, now_ms)?;

        // Safeguard #7: Self-migration no-op guard
        if old_manga_id == new_manga_id {
            return Ok(MigrationReport {
                old_manga_id,
                new_manga_id,
                chapters_matched: 0,
                categories_transferred: old_entry.categories.len(),
                tracks_transferred: 0,
                custom_info_transferred: false,
            });
        }

        // 1. Transfer reader flags & categories (merging with any categories already on new_manga_id)
        if old_entry.reader_flags != 0 {
            self.set_manga_reader_flags(new_manga_id, old_entry.reader_flags, now_ms)?;
        }

        let mut merged_cats: Vec<String> = self
            .get_categories_for_manga(new_manga_id)?
            .into_iter()
            .map(|c| c.name)
            .collect();
        for cat in &old_entry.categories {
            if !merged_cats.contains(cat) {
                merged_cats.push(cat.clone());
            }
        }
        self.set_manga_categories_by_name(new_manga_id, &merged_cats, now_ms)?;

        // 2. Match & transfer chapter reading progress (`read`, `bookmark`, `last_page_read`)
        let old_chapters = self.get_chapters_by_manga_id(old_manga_id)?;
        let target_chapters = self.get_chapters_by_manga_id(new_manga_id)?;

        let mut chapters_matched = 0usize;
        for target_ch in &target_chapters {
            let mut matched_read = target_ch.read;
            let mut matched_bookmark = target_ch.bookmark;
            let mut matched_page = target_ch.last_page_read;
            let mut did_match = false;

            for old_ch in &old_chapters {
                let is_same = if target_ch.chapter.chapter_number >= 0.0
                    && old_ch.chapter.chapter_number >= 0.0
                {
                    (target_ch.chapter.chapter_number - old_ch.chapter.chapter_number).abs() < 1e-3
                } else {
                    !target_ch.chapter.name.trim().is_empty()
                        && target_ch
                            .chapter
                            .name
                            .trim()
                            .eq_ignore_ascii_case(old_ch.chapter.name.trim())
                };

                if is_same && (old_ch.read || old_ch.bookmark || old_ch.last_page_read > 0) {
                    matched_read |= old_ch.read;
                    matched_bookmark |= old_ch.bookmark;
                    matched_page = matched_page.max(old_ch.last_page_read);
                    did_match = true;
                }
            }

            if did_match {
                self.update_chapter_progress(
                    target_ch.id,
                    matched_read,
                    matched_bookmark,
                    matched_page,
                    now_ms,
                )?;
                chapters_matched += 1;
            }
        }

        // 3. Transfer external trackers (`manga_track`)
        let old_tracks = self.get_tracks_for_manga(old_manga_id)?;
        let mut tracks_transferred = 0usize;
        for mut track in old_tracks {
            track.manga_id = new_manga_id;
            self.upsert_manga_track(&track)?;
            tracks_transferred += 1;
        }

        // 4. Transfer Yokai/J2K custom manga metadata (`custom_manga_info`)
        let mut custom_info_transferred = false;
        if let Some(custom_info) = self.get_custom_manga_info(old_manga_id)? {
            self.set_custom_manga_info(new_manga_id, &custom_info)?;
            custom_info_transferred = true;
        }

        // 5. Unfavorite old manga if requested
        if unfavorite_old {
            self.set_manga_favorite(old_manga_id, None, now_ms)?;
        }

        Ok(MigrationReport {
            old_manga_id,
            new_manga_id,
            chapters_matched,
            categories_transferred: merged_cats.len(),
            tracks_transferred,
            custom_info_transferred,
        })
    }

    // =========================================================================
    // YOKAI / J2K PARITY: CUSTOM MANGA INFO, SAVED SEARCHES, DUPLICATE READ & STATS
    // =========================================================================

    /// Saves or updates user-defined metadata overrides (`custom_manga_info`) for `manga_id`.
    pub fn set_custom_manga_info(&self, manga_id: i64, info: &CustomMangaInfo) -> Result<()> {
        if info.is_empty() {
            return self.delete_custom_manga_info(manga_id);
        }
        let genre_json = info
            .genre
            .as_ref()
            .map(|g| serde_json::to_string(g).unwrap_or_else(|_| "[]".to_string()));
        let status_int = info.status.map(|s| s as i32);

        self.conn
            .execute(
                "INSERT INTO custom_manga_info (
                    manga_id, custom_title, custom_author, custom_artist,
                    custom_description, custom_genre, custom_status
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(manga_id) DO UPDATE SET
                    custom_title = excluded.custom_title,
                    custom_author = excluded.custom_author,
                    custom_artist = excluded.custom_artist,
                    custom_description = excluded.custom_description,
                    custom_genre = excluded.custom_genre,
                    custom_status = excluded.custom_status",
                params![
                    manga_id,
                    info.title,
                    info.author,
                    info.artist,
                    info.description,
                    genre_json,
                    status_int,
                ],
            )
            .map_err(|e| IkazuchiError::Database(e.to_string()))?;
        Ok(())
    }

    /// Retrieves user-defined metadata overrides (`custom_manga_info`) for `manga_id`, if any.
    pub fn get_custom_manga_info(&self, manga_id: i64) -> Result<Option<CustomMangaInfo>> {
        let row_opt = self
            .conn
            .query_row(
                "SELECT custom_title, custom_author, custom_artist, custom_description, custom_genre, custom_status
                 FROM custom_manga_info
                 WHERE manga_id = ?1",
                params![manga_id],
                |row| {
                    let genre_str: Option<String> = row.get(4)?;
                    let genre = genre_str
                        .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
                        .filter(|v| !v.is_empty());
                    let status_opt: Option<i32> = row.get(5)?;
                    Ok(CustomMangaInfo {
                        title: row.get(0)?,
                        author: row.get(1)?,
                        artist: row.get(2)?,
                        description: row.get(3)?,
                        genre,
                        status: status_opt.filter(|&v| v != 0).map(MangaStatus::from),
                    })
                },
            )
            .optional()
            .map_err(|e| IkazuchiError::Database(e.to_string()))?;

        Ok(row_opt.filter(|i| !i.is_empty()))
    }

    /// Removes custom metadata overrides for `manga_id`, restoring the source's original metadata.
    pub fn delete_custom_manga_info(&self, manga_id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM custom_manga_info WHERE manga_id = ?1", params![manga_id])
            .map_err(|e| IkazuchiError::Database(e.to_string()))?;
        Ok(())
    }

    /// Marks all duplicate chapters of `manga_id` that share the same `chapter_number` (`>= 0.0`) as read
    /// (matching Yokai 1.10.0's *"Mark duplicate read chapters as read"* feature).
    pub fn mark_duplicate_chapters_read(
        &self,
        manga_id: i64,
        chapter_number: f32,
        now_ms: i64,
    ) -> Result<usize> {
        if chapter_number < 0.0 {
            return Ok(0);
        }
        let affected = self
            .conn
            .execute(
                "UPDATE chapter
                 SET user_read = 1,
                     sync_updated_at = MAX(sync_updated_at, ?3)
                 WHERE manga_id = ?1
                   AND user_read = 0
                   AND ABS(remote_chapter_number - ?2) < 0.001",
                params![manga_id, f64::from(chapter_number), now_ms],
            )
            .map_err(|e| IkazuchiError::Database(e.to_string()))?;
        Ok(affected)
    }

    /// Saves or updates a named catalog search query + filter preset for `source_id` (`saved_search` table).
    pub fn upsert_saved_search(
        &self,
        source_id: i64,
        name: &str,
        query: &str,
        filters: &FilterList,
    ) -> Result<i64> {
        let filters_json = serde_json::to_string(filters).unwrap_or_else(|_| "[]".to_string());
        let trimmed_name = name.trim();
        self.conn
            .execute(
                "INSERT INTO saved_search (source_id, name, query, filters_json)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(source_id, name) DO UPDATE SET
                     query = excluded.query,
                     filters_json = excluded.filters_json",
                params![source_id, trimmed_name, query.trim(), filters_json],
            )
            .map_err(|e| IkazuchiError::Database(e.to_string()))?;

        let id: i64 = self
            .conn
            .query_row(
                "SELECT id FROM saved_search WHERE source_id = ?1 AND name = ?2",
                params![source_id, trimmed_name],
                |r| r.get(0),
            )
            .map_err(|e| IkazuchiError::Database(e.to_string()))?;
        Ok(id)
    }

    /// Lists saved catalog searches, optionally filtered by `source_id`.
    pub fn get_saved_searches(&self, source_id: Option<i64>) -> Result<Vec<SavedSearchRecord>> {
        let sql = if source_id.is_some() {
            "SELECT id, source_id, name, query, filters_json FROM saved_search WHERE source_id = ?1 ORDER BY name COLLATE NOCASE ASC"
        } else {
            "SELECT id, source_id, name, query, filters_json FROM saved_search ORDER BY name COLLATE NOCASE ASC"
        };
        let mut stmt = self
            .conn
            .prepare(sql)
            .map_err(|e| IkazuchiError::Database(e.to_string()))?;

        let map_row = |row: &rusqlite::Row<'_>| -> rusqlite::Result<SavedSearchRecord> {
            let filters_str: String = row.get(4)?;
            let filters: FilterList = serde_json::from_str(&filters_str).unwrap_or_default();
            Ok(SavedSearchRecord {
                id: row.get(0)?,
                source_id: row.get(1)?,
                name: row.get(2)?,
                query: row.get(3)?,
                filters,
            })
        };

        let rows = if let Some(sid) = source_id {
            stmt.query_map(params![sid], map_row)
        } else {
            stmt.query_map([], map_row)
        }
        .map_err(|e| IkazuchiError::Database(e.to_string()))?;

        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| IkazuchiError::Database(e.to_string()))
    }

    /// Deletes a saved catalog search by ID.
    pub fn delete_saved_search(&self, id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM saved_search WHERE id = ?1", params![id])
            .map_err(|e| IkazuchiError::Database(e.to_string()))?;
        Ok(())
    }

    /// Computes aggregated reading and library statistics (Yokai `StatsPresenter.kt`).
    pub fn get_library_stats(&self) -> Result<LibraryStats> {
        let library = self.get_library_manga()?;
        let total_library_mangas = library.len();
        let mut total_chapters = 0usize;
        let mut total_chapters_read = 0usize;
        for item in &library {
            total_chapters += item.total_chapters as usize;
            total_chapters_read += item.total_chapters.saturating_sub(item.unread_count) as usize;
        }

        let total_read_duration_sec: i64 = self
            .conn
            .query_row(
                "SELECT COALESCE(SUM(read_duration_sec), 0) FROM history",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);

        let (tracked_mangas_count, mean_tracker_score): (i64, f64) = self
            .conn
            .query_row(
                "SELECT COUNT(DISTINCT manga_id), COALESCE(AVG(CASE WHEN score > 0 THEN score END), 0.0)
                 FROM manga_track",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap_or((0, 0.0));

        Ok(LibraryStats {
            total_library_mangas,
            total_chapters,
            total_chapters_read,
            total_read_duration_sec,
            tracked_mangas_count: tracked_mangas_count as usize,
            mean_tracker_score,
        })
    }

    /// Returns `(chapter_url, read_at, read_duration_sec)` history entries for a specific `manga_id`.
    pub fn get_history_for_manga(&self, manga_id: i64) -> Result<Vec<BackupHistory>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT C.remote_url, H.read_at, H.read_duration_sec
                 FROM history H
                 JOIN chapter C ON C.id = H.chapter_id
                 WHERE H.manga_id = ?1
                 ORDER BY H.read_at DESC",
            )
            .map_err(|e| IkazuchiError::Database(e.to_string()))?;

        stmt.query_map(params![manga_id], |row| {
            Ok(BackupHistory {
                url: row.get(0)?,
                last_read: row.get(1)?,
                read_duration: row.get(2)?,
            })
        })
        .map_err(|e| IkazuchiError::Database(e.to_string()))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| IkazuchiError::Database(e.to_string()))
    }
}

fn map_library_manga_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<LibraryMangaItem> {
    let genre_str: String = row.get(7)?;
    let genre: Vec<String> = serde_json::from_str(&genre_str).unwrap_or_default();
    let status_int: i32 = row.get(8)?;
    let strategy_int: i32 = row.get(16)?;
    Ok(LibraryMangaItem {
        id: row.get(0)?,
        source_id: row.get(1)?,
        manga: SManga {
            url: row.get(2)?,
            title: row.get(3)?,
            author: row.get(4)?,
            artist: row.get(5)?,
            description: row.get(6)?,
            genre,
            status: MangaStatus::from(status_int),
            thumbnail_url: row.get(9)?,
            update_strategy: UpdateStrategy::from(strategy_int),
            initialized: row.get::<_, i32>(10)? != 0,
        },
        favorite_at: row.get(11)?,
        reader_flags: row.get(12)?,
        sync_updated_at: row.get(13)?,
        unread_count: row.get(14)?,
        total_chapters: row.get(15)?,
        categories: Vec::new(),
    })
}

fn map_chapter_row(row: &rusqlite::Row<'_>, offset: usize) -> rusqlite::Result<ChapterRecord> {
    let ch_num: f64 = row.get(offset + 5)?;
    Ok(ChapterRecord {
        id: row.get(offset)?,
        manga_id: row.get(offset + 1)?,
        chapter: SChapter {
            url: row.get(offset + 2)?,
            name: row.get(offset + 3)?,
            scanlator: row.get(offset + 4)?,
            chapter_number: ch_num as f32,
            date_upload: row.get(offset + 6)?,
        },
        source_order: row.get(offset + 7)?,
        read: row.get::<_, i32>(offset + 8)? != 0,
        bookmark: row.get::<_, i32>(offset + 9)? != 0,
        last_page_read: row.get(offset + 10)?,
        sync_updated_at: row.get(offset + 11)?,
    })
}

fn map_category_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Category> {
    Ok(Category {
        id: row.get(0)?,
        name: row.get(1)?,
        sort_order: row.get(2)?,
        flags: row.get(3)?,
        sync_updated_at: row.get(4)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chapter_sync_preserves_reading_progress() {
        let db = Database::open_in_memory().unwrap();
        let manga = SManga {
            url: "/manga/solo-leveling".into(),
            title: "Solo Leveling".into(),
            status: MangaStatus::Completed,
            initialized: true,
            ..Default::default()
        };

        let manga_id = db.upsert_manga(42, &manga, Some(1000), 1000).unwrap();

        let initial_chapters = vec![
            SChapter {
                url: "/ch/2".into(),
                name: "Chapter 2".into(),
                chapter_number: 2.0,
                date_upload: 2000,
                scanlator: Some("Asura".into()),
            },
            SChapter {
                url: "/ch/1".into(),
                name: "Chapter 1".into(),
                chapter_number: 1.0,
                date_upload: 1000,
                scanlator: Some("Asura".into()),
            },
        ];

        db.sync_chapters(manga_id, &initial_chapters, 2000).unwrap();
        let stored = db.get_chapters_by_manga_id(manga_id).unwrap();
        assert_eq!(stored.len(), 2);

        let ch1_id = stored[1].id;
        let ch2_id = stored[0].id;
        db.update_chapter_progress(ch1_id, true, false, 35, 3000).unwrap();
        db.update_chapter_progress(ch2_id, false, true, 11, 3500).unwrap();

        let updated_chapters = vec![
            SChapter {
                url: "/ch/3".into(),
                name: "Chapter 3 - New!".into(),
                chapter_number: 3.0,
                date_upload: 4000,
                scanlator: Some("Asura".into()),
            },
            SChapter {
                url: "/ch/2".into(),
                name: "Chapter 2 (v2)".into(),
                chapter_number: 2.0,
                date_upload: 2000,
                scanlator: Some("Asura".into()),
            },
            initial_chapters[1].clone(),
        ];

        db.sync_chapters(manga_id, &updated_chapters, 4000).unwrap();
        let after_sync = db.get_chapters_by_manga_id(manga_id).unwrap();
        assert_eq!(after_sync.len(), 3);
        assert!(!after_sync[0].read);
        assert_eq!(after_sync[1].chapter.name, "Chapter 2 (v2)");
        assert!(after_sync[1].bookmark);
        assert_eq!(after_sync[1].last_page_read, 11);
        assert!(after_sync[2].read);
    }

    #[test]
    fn test_categories_and_history_queries() {
        let db = Database::open_in_memory().unwrap();
        let manga = SManga {
            url: "/manga/berserk".into(),
            title: "Berserk".into(),
            status: MangaStatus::Ongoing,
            initialized: true,
            ..Default::default()
        };
        let manga_id = db.upsert_manga(7, &manga, Some(500), 500).unwrap();
        db.set_manga_categories_by_name(manga_id, &["Dark Fantasy".into(), "Seinen".into()], 600)
            .unwrap();

        let lib = db.get_library_manga().unwrap();
        assert_eq!(lib.len(), 1);
        assert_eq!(lib[0].categories, vec!["Dark Fantasy", "Seinen"]);
        assert_eq!(lib[0].unread_count, 0);

        let chapters = vec![SChapter {
            url: "/ch/376".into(),
            name: "Chapter 376".into(),
            chapter_number: 376.0,
            date_upload: 700,
            scanlator: Some("Evil_Genius".into()),
        }];
        db.sync_chapters(manga_id, &chapters, 700).unwrap();
        let ch_id = db.get_chapters_by_manga_id(manga_id).unwrap()[0].id;
        db.update_chapter_progress(ch_id, false, true, 9, 800).unwrap();

        let history = db.get_recent_history(10).unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].manga_title, "Berserk");
        assert_eq!(history[0].last_page_read, 9);

        let updates = db.get_recent_updates(10).unwrap();
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].chapter.chapter.chapter_number, 376.0);
    }

    #[test]
    fn test_mihon_tachibk_backup_import_and_export_with_all_safeguards() {
        let db = Database::open_in_memory().unwrap();

        let synthetic_backup = MihonBackup {
            backup_categories: vec![
                BackupCategory {
                    name: "Seinen".into(),
                    order: 0,
                    id: 1,
                    flags: 0,
                },
                BackupCategory {
                    name: "Masterpieces".into(),
                    order: 5,
                    id: 2,
                    flags: 0,
                },
            ],
            backup_manga: vec![BackupManga {
                source: 2499283573021220255,
                // Full URL to test Safeguard #8 (domain stripping)
                url: "https://mangadex.org/title/vagabond".into(),
                title: "Vagabond".into(),
                author: Some("Takehiko Inoue".into()),
                artist: Some("Takehiko Inoue".into()),
                description: Some("Musashi chronicle".into()),
                genre: vec!["Historical".into(), "Seinen".into()],
                status: 6, // OnHiatus
                thumbnail_url: None,
                // date_added = 0, favorite_modified_at in seconds -> Safeguard #4
                date_added: 0,
                favorite_modified_at: Some(1_700_000_000),
                viewer: 1,
                viewer_flags: Some(3),
                // Duplicate chapter URLs + negative last_page_read -> Safeguards #3 & #4
                chapters: vec![
                    BackupChapter {
                        url: "https://mangadex.org/ch/1".into(),
                        name: "Chapter 1".into(),
                        scanlator: Some("Official".into()),
                        read: false,
                        bookmark: false,
                        last_page_read: -99,
                        date_fetch: 1000,
                        date_upload: 1000,
                        chapter_number: 1.0,
                        source_order: 1,
                    },
                    BackupChapter {
                        url: "/ch/1".into(),
                        name: "Chapter 1".into(),
                        scanlator: Some("Official".into()),
                        read: true,
                        bookmark: true,
                        last_page_read: 42,
                        date_fetch: 2000,
                        date_upload: 1000,
                        chapter_number: 1.0,
                        source_order: 1,
                    },
                    BackupChapter {
                        url: "/ch/2".into(),
                        name: "Chapter 2 (MTL)".into(),
                        scanlator: Some("SpamScanlator".into()),
                        read: false,
                        bookmark: false,
                        last_page_read: 0,
                        date_fetch: 3000,
                        date_upload: 3000,
                        chapter_number: 2.0,
                        source_order: 0,
                    },
                ],
                categories: vec![0, 5],
                tracking: vec![BackupTracking {
                    sync_id: 2, // AniList
                    library_id: 777,
                    media_id_int: 12345, // Legacy int media ID
                    tracking_url: "https://anilist.co/manga/12345".into(),
                    title: "Vagabond".into(),
                    last_chapter_read: 1.0,
                    total_chapters: 327,
                    score: 9.5,
                    status: 1,
                    started_reading_date: 1000,
                    finished_reading_date: 0,
                    private: false,
                    media_id: 0,
                }],
                favorite: true,
                chapter_flags: 0,
                history: vec![BackupHistory {
                    url: "/ch/1".into(),
                    last_read: 5000,
                    read_duration: 180,
                }],
                update_strategy: 1, // OnlyFetchOnce
                excluded_scanlators: vec!["SpamScanlator".into()],
                notes: String::new(),
                initialized: true,
                ..Default::default()
            }],
            backup_sources: vec![BackupSource {
                name: "MangaDex".into(),
                source_id: 2499283573021220255,
            }],
            backup_extension_stores: vec![BackupExtensionStore {
                index_url: "https://raw.githubusercontent.com/keiyoushi/extensions/repo/index.min.json".into(),
                name: "Keiyoushi".into(),
                badge_label: None,
                contact_website: String::new(),
                signing_key: "abc123".into(),
                contact_discord: None,
                is_legacy: Some(false),
                extension_list_url: None,
            }],
        };

        let tachibk_gz = encode_mihon_backup_bytes(&synthetic_backup, true).unwrap();
        // Verify Gzip magic header
        assert_eq!(&tachibk_gz[0..2], &[0x1F, 0x8B]);

        let report = db.import_mihon_backup_bytes(&tachibk_gz, 9999).unwrap();
        assert_eq!(report.categories_restored, 2);
        assert_eq!(report.mangas_restored, 1);
        assert_eq!(report.chapters_restored, 2); // Duplicate /ch/1 coalesced!
        assert_eq!(report.history_entries_restored, 1);
        assert_eq!(report.tracks_restored, 1);
        assert_eq!(report.excluded_scanlators_restored, 1);
        assert_eq!(report.extension_stores_restored, 1);

        let lib = db.get_library_manga().unwrap();
        assert_eq!(lib.len(), 1);
        assert_eq!(lib[0].manga.url, "/title/vagabond");
        assert_eq!(lib[0].favorite_at, 1_700_000_000_000);
        assert_eq!(lib[0].reader_flags, 3);
        assert_eq!(lib[0].manga.update_strategy, UpdateStrategy::OnlyFetchOnce);
        assert_eq!(lib[0].categories, vec!["Seinen", "Masterpieces"]);
        // Chapter 2 is from "SpamScanlator" (excluded!), and Chapter 1 is read -> unread_count MUST be 0!
        assert_eq!(lib[0].unread_count, 0);
        assert_eq!(lib[0].total_chapters, 1);
        // Storm feed (`get_recent_updates`) must also exclude "SpamScanlator"!
        assert_eq!(db.get_recent_updates(10).unwrap().len(), 1);

        let visible_chs = db.get_visible_chapters_by_manga_id(lib[0].id).unwrap();
        assert_eq!(visible_chs.len(), 1);
        assert!(visible_chs[0].read);
        assert!(visible_chs[0].bookmark);
        assert_eq!(visible_chs[0].last_page_read, 42);

        let tracks = db.get_tracks_for_manga(lib[0].id).unwrap();
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].remote_id, 12345);

        // Round-trip export from DB -> import into a fresh DB2
        let exported_gz = db.export_mihon_backup_bytes(true).unwrap();
        let db2 = Database::open_in_memory().unwrap();
        let report2 = db2.import_mihon_backup_bytes(&exported_gz, 10000).unwrap();
        assert_eq!(report2.mangas_restored, 1);
        assert_eq!(report2.history_entries_restored, 1);
        let lib2 = db2.get_library_manga().unwrap();
        assert_eq!(lib2[0].manga.title, "Vagabond");
        assert_eq!(lib2[0].manga.update_strategy, UpdateStrategy::OnlyFetchOnce);
    }

    #[test]
    fn test_migrate_manga_and_excluded_scanlators_safeguards() {
        let db = Database::open_in_memory().unwrap();
        let old_manga = SManga {
            url: "/manga/old-northern-blade".into(),
            title: "Legend of the Northern Blade".into(),
            status: MangaStatus::Ongoing,
            initialized: true,
            ..Default::default()
        };
        let old_id = db.upsert_manga(100, &old_manga, Some(1000), 1000).unwrap();
        db.set_manga_categories_by_name(old_id, &["Murim".into()], 1000).unwrap();
        db.set_manga_reader_flags(old_id, 7, 1000).unwrap();

        let old_chs = vec![
            SChapter {
                url: "/old/extra-1".into(),
                name: "Season 1 Recap".into(),
                chapter_number: -1.0,
                date_upload: 1200,
                scanlator: None,
            },
            SChapter {
                url: "/old/ch-1".into(),
                name: "Chapter 1".into(),
                chapter_number: 1.0,
                date_upload: 1000,
                scanlator: None,
            },
        ];
        db.sync_chapters(old_id, &old_chs, 1200).unwrap();
        let stored_old = db.get_chapters_by_manga_id(old_id).unwrap();
        // Mark both old chapters read
        db.update_chapter_progress(stored_old[0].id, true, false, 10, 1300).unwrap();
        db.update_chapter_progress(stored_old[1].id, true, true, 55, 1300).unwrap();

        db.upsert_manga_track(&MangaTrackRecord {
            id: 0,
            manga_id: old_id,
            tracker_id: 1,
            remote_id: 99999,
            library_id: None,
            title: "Northern Blade".into(),
            last_chapter_read: 1.0,
            total_chapters: 202,
            status: 1,
            score: 10.0,
            remote_url: String::new(),
            start_date: 1000,
            finish_date: 0,
            private: false,
        })
        .unwrap();

        // Safeguard #7a: Self-migration no-op guard must NOT unfavorite old_id
        let self_mig = db
            .migrate_manga(old_id, 100, &old_manga, &old_chs, true, 1500)
            .unwrap();
        assert_eq!(self_mig.old_manga_id, self_mig.new_manga_id);
        assert_eq!(db.get_library_manga().unwrap().len(), 1);

        // Migrate from Source 100 -> Source 200
        let new_manga = SManga {
            url: "/series/northern-blade-new".into(),
            title: "Legend of the Northern Blade".into(),
            status: MangaStatus::Completed,
            initialized: true,
            ..Default::default()
        };
        let new_chs = vec![
            SChapter {
                url: "/new/ch-2".into(),
                name: "Chapter 2".into(),
                chapter_number: 2.0,
                date_upload: 2000,
                scanlator: Some("Asura".into()),
            },
            // Unrelated -1.0 special must NOT falsely match "Season 1 Recap" -> Safeguard #7b
            SChapter {
                url: "/new/afterword".into(),
                name: "Author Afterword".into(),
                chapter_number: -1.0,
                date_upload: 1900,
                scanlator: Some("Asura".into()),
            },
            SChapter {
                url: "/new/ch-1".into(),
                name: "Episode 1".into(),
                chapter_number: 1.0,
                date_upload: 1000,
                scanlator: Some("Asura".into()),
            },
        ];

        let mig_report = db
            .migrate_manga(old_id, 200, &new_manga, &new_chs, true, 2000)
            .unwrap();
        assert_eq!(mig_report.chapters_matched, 1); // Only Ch 1.0 matched, NOT "Author Afterword" (-1.0)!
        assert_eq!(mig_report.categories_transferred, 1);
        assert_eq!(mig_report.tracks_transferred, 1);

        let lib = db.get_library_manga().unwrap();
        assert_eq!(lib.len(), 1);
        assert_eq!(lib[0].id, mig_report.new_manga_id);
        assert_eq!(lib[0].categories, vec!["Murim"]);
        assert_eq!(lib[0].reader_flags, 7);

        let migrated_chs = db.get_chapters_by_manga_id(mig_report.new_manga_id).unwrap();
        assert!(!migrated_chs[0].read); // Ch 2.0 unread
        assert!(!migrated_chs[1].read); // "Author Afterword" (-1.0) stayed unread!
        assert!(migrated_chs[2].read); // Ch 1.0 transferred read = true!
        assert!(migrated_chs[2].bookmark);
        assert_eq!(migrated_chs[2].last_page_read, 55);
    }

    #[test]
    fn test_yokai_custom_manga_info_saved_searches_duplicate_read_and_stats() {
        let db = Database::open_in_memory().unwrap();
        let manga = SManga {
            url: "/manga/omniscient-reader".into(),
            title: "Omniscient Reader's Viewpoint (Raw Title)".into(),
            author: Some("Sing N Song".into()),
            artist: Some("Sleepy-C".into()),
            description: Some("Original source synopsis.".into()),
            genre: vec!["Action".into()],
            status: MangaStatus::Ongoing,
            initialized: true,
            ..Default::default()
        };
        let manga_id = db.upsert_manga(300, &manga, Some(1000), 1000).unwrap();

        // 1. Apply Yokai custom metadata overrides
        let custom = CustomMangaInfo {
            title: Some("Omniscient Reader".into()),
            author: None,
            artist: None,
            description: Some("Custom user-curated synopsis.".into()),
            genre: Some(vec!["Action".into(), "Apocalyptic".into(), "Regression".into()]),
            status: Some(MangaStatus::Completed),
        };
        db.set_custom_manga_info(manga_id, &custom).unwrap();

        let item = db.get_manga_by_id(manga_id).unwrap().unwrap();
        assert_eq!(item.manga.title, "Omniscient Reader");
        assert_eq!(item.manga.author.as_deref(), Some("Sing N Song")); // Inherited from remote
        assert_eq!(
            item.manga.description.as_deref(),
            Some("Custom user-curated synopsis.")
        );
        assert_eq!(
            item.manga.genre,
            vec!["Action", "Apocalyptic", "Regression"]
        );
        assert_eq!(item.manga.status, MangaStatus::Completed);

        // Source refresh must NOT overwrite user's CustomMangaInfo!
        let refreshed_remote = SManga {
            title: "Changed By Source Website".into(),
            ..manga.clone()
        };
        db.upsert_manga(300, &refreshed_remote, Some(1000), 2000).unwrap();
        let lib = db.get_library_manga().unwrap();
        assert_eq!(lib[0].manga.title, "Omniscient Reader");

        // 2. Round-trip Yokai `.tachibk` Protobuf tags 602, 800..805
        let exported_gz = db.export_mihon_backup_bytes(true).unwrap();
        let db2 = Database::open_in_memory().unwrap();
        db2.import_mihon_backup_bytes(&exported_gz, 3000).unwrap();
        let lib2 = db2.get_library_manga().unwrap();
        assert_eq!(lib2[0].manga.title, "Omniscient Reader");
        assert_eq!(lib2[0].manga.status, MangaStatus::Completed);
        assert_eq!(
            lib2[0].manga.genre,
            vec!["Action", "Apocalyptic", "Regression"]
        );

        // 3. Auto-mark duplicate chapters read across scanlators
        let chs = vec![
            SChapter {
                url: "/orv/ch-10-asura".into(),
                name: "Chapter 10 (Asura)".into(),
                chapter_number: 10.0,
                date_upload: 1000,
                scanlator: Some("Asura".into()),
            },
            SChapter {
                url: "/orv/ch-10-flame".into(),
                name: "Chapter 10 (Flame)".into(),
                chapter_number: 10.0,
                date_upload: 1000,
                scanlator: Some("Flame".into()),
            },
            SChapter {
                url: "/orv/ch-11".into(),
                name: "Chapter 11".into(),
                chapter_number: 11.0,
                date_upload: 2000,
                scanlator: Some("Asura".into()),
            },
        ];
        db.sync_chapters(manga_id, &chs, 2500).unwrap();
        let stored_chs = db.get_chapters_by_manga_id(manga_id).unwrap();
        // Read Ch 10 (Asura) and record 600s history + mark duplicate Ch 10 (Flame) read
        db.update_chapter_progress(stored_chs[1].id, true, false, 40, 2600)
            .unwrap();
        db.upsert_history_entry(stored_chs[1].id, 2600, 600).unwrap();
        let dup_marked = db
            .mark_duplicate_chapters_read(manga_id, 10.0, 2600)
            .unwrap();
        assert_eq!(dup_marked, 1);

        let after_dup = db.get_chapters_by_manga_id(manga_id).unwrap();
        assert!(after_dup[0].read); // Ch 10 (Asura) auto-marked read!
        assert!(after_dup[1].read); // Ch 10 (Flame) read
        assert!(!after_dup[2].read); // Ch 11 unread

        // 4. Saved Searches
        let filters = vec![ikazuchi_core::Filter::Text {
            name: "Author".into(),
            state: "Sing N Song".into(),
        }];
        db.upsert_saved_search(300, "Regression Hits", "omniscient", &filters)
            .unwrap();
        let saved = db.get_saved_searches(Some(300)).unwrap();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].name, "Regression Hits");
        assert_eq!(saved[0].query, "omniscient");
        assert_eq!(saved[0].filters, filters);

        // 5. Library & Reading Statistics
        let stats = db.get_library_stats().unwrap();
        assert_eq!(stats.total_library_mangas, 1);
        assert_eq!(stats.total_chapters, 3);
        assert_eq!(stats.total_chapters_read, 2);
        assert_eq!(stats.total_read_duration_sec, 600);
    }
}
