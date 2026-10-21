//! # Ikazuchi Application State & Raijin Reader Session Controller
//!
//! Coordinates navigation across the 5 Realms of Ikazuchi (*Sanctuary*, *Storm*,
//! *Horizon*, *Chronicle*, *Vault*) and manages the *Raijin Reader* state machine.

use ikazuchi_core::{
    AppConfig, LibraryGroupMode, MangaStatus, Page, ReaderSettings, ReadingModeSetting, SChapter,
    SManga, generate_source_id,
};
use ikazuchi_db::{
    ChapterRecord, Database, HistoryItem, LibraryMangaItem, LibraryStats, RecentUpdateItem,
    SavedSearchRecord,
};
use ikazuchi_dex::{ExtensionManager, MadaraSource, MangaDexSource, MangaThemesiaSource};
use ikazuchi_sync::{GithubSyncConfig, LibraryIndex, export_database_shards};
use std::collections::BTreeMap;
use std::sync::Arc;

/// The 5 Realms of Ikazuchi + the Raijin Reader overlay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Realm {
    /// 1. Library Grid, Categories, Unread Badges
    Sanctuary,
    /// 2. Recent Chapter Drops & Reading Timeline
    Storm,
    /// 3. Catalog Browser & 1-Click Mihon Extension Installer
    Horizon,
    /// 4. Manga Details, Synopsis, Scanlator Filter & Chapter List
    Chronicle { manga_id: i64 },
    /// 5. Serverless Private GitHub Repo Sync & Settings
    Vault,
    /// ⚡ Raijin Fullscreen Reader (Webtoon Continuous & Paged RTL/LTR)
    RaijinReader { manga_id: i64, chapter_id: i64 },
}

/// Reading mode supported by the Raijin Reader Engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReaderMode {
    /// Gapless vertical strip for Manhwa / Webtoons
    WebtoonContinuous,
    /// Right-to-Left paged view for traditional Japanese Manga
    PagedRtl,
    /// Left-to-Right paged view for Western Comics
    PagedLtr,
}

impl ReaderMode {
    pub fn label(&self) -> &'static str {
        match self {
            Self::WebtoonContinuous => "WEBTOON // VERTICAL STRIP",
            Self::PagedRtl => "PAGED // RIGHT-TO-LEFT",
            Self::PagedLtr => "PAGED // LEFT-TO-RIGHT",
        }
    }

    pub fn cycle_next(&self) -> Self {
        match self {
            Self::WebtoonContinuous => Self::PagedRtl,
            Self::PagedRtl => Self::PagedLtr,
            Self::PagedLtr => Self::WebtoonContinuous,
        }
    }

    pub fn from_setting(setting: ReadingModeSetting) -> Self {
        match setting {
            ReadingModeSetting::WebtoonContinuous => Self::WebtoonContinuous,
            ReadingModeSetting::PagedRtl => Self::PagedRtl,
            ReadingModeSetting::PagedLtr => Self::PagedLtr,
        }
    }
}

/// Active reading session in the Raijin Reader Engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReaderSession {
    pub manga_id: i64,
    pub manga_title: String,
    pub chapter_id: i64,
    pub chapter_name: String,
    pub pages: Vec<Page>,
    pub current_page: usize,
    pub mode: ReaderMode,
    pub zoom_width_pct: u32,
    pub hud_visible: bool,
}

impl ReaderSession {
    pub fn new(
        manga_id: i64,
        manga_title: String,
        chapter_id: i64,
        chapter_name: String,
        pages: Vec<Page>,
        start_page: usize,
        mode: ReaderMode,
    ) -> Self {
        let clamped = if pages.is_empty() {
            0
        } else {
            start_page.min(pages.len() - 1)
        };
        Self {
            manga_id,
            manga_title,
            chapter_id,
            chapter_name,
            pages,
            current_page: clamped,
            mode,
            zoom_width_pct: 65,
            hud_visible: true,
        }
    }

    /// Creates a [`ReaderSession`] initialized with the user's [`ReaderSettings`].
    pub fn from_settings(
        manga_id: i64,
        manga_title: String,
        chapter_id: i64,
        chapter_name: String,
        pages: Vec<Page>,
        start_page: usize,
        settings: &ReaderSettings,
    ) -> Self {
        let mut session = Self::new(
            manga_id,
            manga_title,
            chapter_id,
            chapter_name,
            pages,
            start_page,
            ReaderMode::from_setting(settings.default_reading_mode),
        );
        session.zoom_width_pct = settings.default_zoom_width_pct.clamp(30, 100);
        session
    }

    /// Advances to the next page; returns `true` if the user reached the last page of the chapter.
    pub fn next_page(&mut self) -> bool {
        if self.pages.is_empty() {
            return false;
        }
        if self.current_page + 1 < self.pages.len() {
            self.current_page += 1;
        }
        self.is_on_last_page()
    }

    /// Moves back one page.
    pub fn prev_page(&mut self) {
        if self.current_page > 0 {
            self.current_page -= 1;
        }
    }

    pub fn is_on_last_page(&self) -> bool {
        !self.pages.is_empty() && self.current_page + 1 == self.pages.len()
    }

    pub fn zoom_in(&mut self) {
        self.zoom_width_pct = (self.zoom_width_pct + 10).min(100);
    }

    pub fn zoom_out(&mut self) {
        self.zoom_width_pct = self.zoom_width_pct.saturating_sub(10).max(30);
    }
}

/// Snapshot of data rendered across the Dioxus UI realms.
pub struct AppSnapshot {
    pub library: Vec<LibraryMangaItem>,
    pub categories: Vec<String>,
    pub updates: Vec<RecentUpdateItem>,
    pub history: Vec<HistoryItem>,
    pub installed_sources: Vec<(i64, String, String, String)>,
    pub saved_searches: Vec<SavedSearchRecord>,
    pub stats: LibraryStats,
    pub sync_index: LibraryIndex,
    pub github_config: GithubSyncConfig,
    pub config: AppConfig,
}

impl AppSnapshot {
    /// Initializes an `AppSnapshot` backed by `Database` and `ExtensionManager` using default settings.
    pub fn load_from(db: &Database, ext_mgr: &ExtensionManager) -> Self {
        Self::load_from_with_config(db, ext_mgr, AppConfig::default())
    }

    /// Initializes an `AppSnapshot` backed by `Database`, `ExtensionManager`, and persistent `AppConfig`
    /// (automatically filtering installed sources by NSFW, language, disabled, and pinned preferences).
    pub fn load_from_with_config(
        db: &Database,
        ext_mgr: &ExtensionManager,
        config: AppConfig,
    ) -> Self {
        let library = db.get_library_manga().unwrap_or_default();
        let categories = db
            .get_categories()
            .unwrap_or_default()
            .into_iter()
            .map(|c| c.name)
            .collect();
        let updates = db.get_recent_updates(25).unwrap_or_default();
        let history = db.get_recent_history(25).unwrap_or_default();
        let saved_searches = db.get_saved_searches(None).unwrap_or_default();
        let stats = db.get_library_stats().unwrap_or_default();

        let installed_sources: Vec<(i64, String, String, String)> = ext_mgr
            .visible_sources(&config.extensions)
            .into_iter()
            .map(|s| {
                (
                    s.id(),
                    s.name().to_string(),
                    s.lang().to_string(),
                    s.base_url().to_string(),
                )
            })
            .collect();

        let (sync_index, _) = export_database_shards(db).unwrap_or_default();

        let github_config = GithubSyncConfig {
            owner: config.backup_sync.github_owner.clone(),
            repo: config.backup_sync.github_repo.clone(),
            branch: config.backup_sync.github_branch.clone(),
            personal_access_token: config.backup_sync.github_personal_access_token.clone(),
            api_base_url: config.backup_sync.github_api_base_url.clone(),
        };

        Self {
            library,
            categories,
            updates,
            history,
            installed_sources,
            saved_searches,
            stats,
            sync_index,
            github_config,
            config,
        }
    }

    /// Groups library entries dynamically according to `self.config.library.group_mode`
    /// (Yokai `LibraryGroup.kt` parity: `ByCategory`, `ByStatus`, `BySource`, `ByGenre`, `ByAuthor`, `Ungrouped`).
    pub fn grouped_library(&self) -> Vec<(String, Vec<&LibraryMangaItem>)> {
        let mut groups: BTreeMap<String, Vec<&LibraryMangaItem>> = BTreeMap::new();

        match self.config.library.group_mode {
            LibraryGroupMode::Ungrouped => {
                return vec![("All Realms".to_string(), self.library.iter().collect())];
            }
            LibraryGroupMode::ByCategory => {
                for item in &self.library {
                    if item.categories.is_empty() {
                        groups.entry("Uncategorized".to_string()).or_default().push(item);
                    } else {
                        for cat in &item.categories {
                            groups.entry(cat.clone()).or_default().push(item);
                        }
                    }
                }
            }
            LibraryGroupMode::ByStatus => {
                for item in &self.library {
                    let label = match item.manga.status {
                        MangaStatus::Ongoing => "Ongoing",
                        MangaStatus::Completed => "Completed",
                        MangaStatus::Licensed => "Licensed",
                        MangaStatus::PublishingFinished => "Publishing Finished",
                        MangaStatus::Cancelled => "Cancelled",
                        MangaStatus::OnHiatus => "On Hiatus",
                        MangaStatus::Unknown => "Unknown",
                    };
                    groups.entry(label.to_string()).or_default().push(item);
                }
            }
            LibraryGroupMode::BySource => {
                for item in &self.library {
                    let source_name = self
                        .installed_sources
                        .iter()
                        .find(|(id, _, _, _)| *id == item.source_id)
                        .map(|(_, name, _, _)| name.clone())
                        .unwrap_or_else(|| {
                            if item.source_id == 0 {
                                "Local Source".to_string()
                            } else {
                                format!("Source #{}", item.source_id)
                            }
                        });
                    groups.entry(source_name).or_default().push(item);
                }
            }
            LibraryGroupMode::ByGenre => {
                for item in &self.library {
                    if item.manga.genre.is_empty() {
                        groups.entry("No Genre".to_string()).or_default().push(item);
                    } else {
                        for g in &item.manga.genre {
                            groups.entry(g.clone()).or_default().push(item);
                        }
                    }
                }
            }
            LibraryGroupMode::ByAuthor => {
                for item in &self.library {
                    let author = item
                        .manga
                        .author
                        .as_deref()
                        .or(item.manga.artist.as_deref())
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .unwrap_or("Unknown Author");
                    groups.entry(author.to_string()).or_default().push(item);
                }
            }
        }

        groups.into_iter().collect()
    }

    pub fn get_chapters(db: &Database, manga_id: i64) -> Vec<ChapterRecord> {
        db.get_visible_chapters_by_manga_id(manga_id).unwrap_or_default()
    }
}

/// Seeds initial demo library & extension data into `db` and `ext_mgr` if the library is empty.
pub fn seed_initial_sanctuary(db: &Database, ext_mgr: &mut ExtensionManager) {
    let md = Arc::new(MangaDexSource::new("en"));
    let asura = Arc::new(MangaThemesiaSource::new("Asura Scans", "https://asuracomic.net", "en", 1));
    let leviatan = Arc::new(MadaraSource::new("Leviatan Scans", "https://leviatanscans.com", "en", 1));

    ext_mgr.register_source(md.clone());
    ext_mgr.register_source(asura.clone());
    ext_mgr.register_source(leviatan.clone());

    let _ = db.upsert_extension_store(
        "https://raw.githubusercontent.com/keiyoushi/extensions/repo/index.json",
        "Keiyoushi Official Mihon Repo",
        "9add655a78e96c4ec7a53ef89dccb557cb5d767489fac5e785d671a5a75d4da2",
    );

    if !db.get_library_manga().unwrap_or_default().is_empty() {
        return;
    }

    let entries = [
        (
            generate_source_id("MangaDex", "en", 1),
            SManga {
                url: "/manga/vagabond".into(),
                title: "Vagabond".into(),
                author: Some("Takehiko Inoue".into()),
                artist: Some("Takehiko Inoue".into()),
                description: Some("Chronicle of Shinmen Takezo (Miyamoto Musashi) on the path of the sword.".into()),
                genre: vec!["Seinen".into(), "Historical".into(), "Martial Arts".into()],
                status: MangaStatus::OnHiatus,
                thumbnail_url: None,
                update_strategy: Default::default(),
                initialized: true,
            },
            vec!["Manga".to_string(), "Seinen".to_string()],
            vec![
                ("Chapter 327 - The Man Named Tadaoki", 327.0, false, 0),
                ("Chapter 326 - Nuance", 326.0, true, 24),
                ("Chapter 325", 325.0, true, 22),
            ],
        ),
        (
            generate_source_id("Asura Scans", "en", 1),
            SManga {
                url: "/manga/legend-of-the-northern-blade".into(),
                title: "Legend of the Northern Blade".into(),
                author: Some("Woogack".into()),
                artist: Some("Hae-Min".into()),
                description: Some("After the fall of the Northern Heavenly Sect, Jin Mu-Won rises from the ashes.".into()),
                genre: vec!["Manhwa".into(), "Murim".into(), "Action".into()],
                status: MangaStatus::Completed,
                thumbnail_url: None,
                update_strategy: Default::default(),
                initialized: true,
            },
            vec!["Manhwa".to_string(), "Reading".to_string()],
            vec![
                ("Chapter 202 - Finale", 202.0, false, 14),
                ("Chapter 201", 201.0, true, 60),
            ],
        ),
        (
            generate_source_id("MangaDex", "en", 1),
            SManga {
                url: "/manga/berserk".into(),
                title: "Berserk".into(),
                author: Some("Kentaro Miura".into()),
                artist: Some("Kentaro Miura".into()),
                description: Some("Guts, the Black Swordsman, defies fate in a dark medieval world.".into()),
                genre: vec!["Seinen".into(), "Dark Fantasy".into()],
                status: MangaStatus::Ongoing,
                thumbnail_url: None,
                update_strategy: Default::default(),
                initialized: true,
            },
            vec!["Manga".to_string(), "Seinen".to_string()],
            vec![
                ("Chapter 376", 376.0, false, 0),
                ("Chapter 375", 375.0, false, 8),
            ],
        ),
    ];

    let now_ms = 1_729_520_000_000;
    for (idx, (source_id, manga, cats, ch_list)) in entries.into_iter().enumerate() {
        let fav_ts = now_ms + (idx as i64) * 1000;
        if let Ok(manga_id) = db.upsert_manga(source_id, &manga, Some(fav_ts), fav_ts) {
            let _ = db.set_manga_categories_by_name(manga_id, &cats, fav_ts);
            let schapters: Vec<SChapter> = ch_list
                .iter()
                .map(|(name, num, _, _)| SChapter {
                    url: format!("{}/ch-{num}", manga.url),
                    name: (*name).to_string(),
                    chapter_number: *num,
                    date_upload: fav_ts,
                    scanlator: Some("Official".into()),
                })
                .collect();
            let _ = db.sync_chapters(manga_id, &schapters, fav_ts);
            let stored = db.get_chapters_by_manga_id(manga_id).unwrap_or_default();
            for (s_ch, (_, _, is_read, last_page)) in stored.iter().zip(ch_list.iter()) {
                if *is_read || *last_page > 0 {
                    let _ = db.update_chapter_progress(s_ch.id, *is_read, !*is_read, *last_page, fav_ts + 500);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_raijin_reader_session_state_transitions() {
        let pages = vec![
            Page {
                index: 0,
                url: String::new(),
                image_url: Some("https://cdn.example.org/1.webp".into()),
            },
            Page {
                index: 1,
                url: String::new(),
                image_url: Some("https://cdn.example.org/2.webp".into()),
            },
            Page {
                index: 2,
                url: String::new(),
                image_url: Some("https://cdn.example.org/3.webp".into()),
            },
        ];

        let mut session = ReaderSession::new(
            1,
            "Legend of the Northern Blade".into(),
            10,
            "Chapter 202".into(),
            pages,
            0,
            ReaderMode::WebtoonContinuous,
        );

        assert!(!session.is_on_last_page());
        assert!(!session.next_page());
        assert_eq!(session.current_page, 1);
        assert!(session.next_page());
        assert_eq!(session.current_page, 2);
        assert!(session.is_on_last_page());

        session.prev_page();
        assert_eq!(session.current_page, 1);

        session.zoom_in();
        assert_eq!(session.zoom_width_pct, 75);
        assert_eq!(session.mode.cycle_next(), ReaderMode::PagedRtl);
    }

    #[test]
    fn test_yokai_dynamic_library_grouping_modes() {
        let db = Database::open_in_memory().unwrap();
        let mut ext_mgr = ExtensionManager::new();
        seed_initial_sanctuary(&db, &mut ext_mgr);

        let mut cfg = AppConfig::default();

        // 1. Group by Status
        cfg.library.group_mode = LibraryGroupMode::ByStatus;
        let snap_status = AppSnapshot::load_from_with_config(&db, &ext_mgr, cfg.clone());
        let status_groups = snap_status.grouped_library();
        assert!(status_groups.iter().any(|(k, _)| k == "Completed"));
        assert!(status_groups.iter().any(|(k, _)| k == "Ongoing"));
        assert!(status_groups.iter().any(|(k, _)| k == "On Hiatus"));

        // 2. Group by Author
        cfg.library.group_mode = LibraryGroupMode::ByAuthor;
        let snap_author = AppSnapshot::load_from_with_config(&db, &ext_mgr, cfg.clone());
        let author_groups = snap_author.grouped_library();
        assert!(author_groups.iter().any(|(k, _)| k == "Takehiko Inoue"));
        assert!(author_groups.iter().any(|(k, _)| k == "Kentaro Miura"));

        // 3. Group by Source
        cfg.library.group_mode = LibraryGroupMode::BySource;
        let snap_source = AppSnapshot::load_from_with_config(&db, &ext_mgr, cfg);
        let source_groups = snap_source.grouped_library();
        assert!(source_groups.iter().any(|(k, _)| k == "MangaDex"));
        assert!(source_groups.iter().any(|(k, _)| k == "Asura Scans"));
    }
}
