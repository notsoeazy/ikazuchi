//! # Application Settings & Persistent Config File Backend (`config.json`)
//!
//! Models Mihon's 6 preference domains (`UiPreferences`, `LibraryPreferences`, `ReaderPreferences`,
//! `SourcePreferences`, `DownloadPreferences`, `BackupPreferences`) in a single strongly-typed,
//! forward/backward-compatible JSON configuration file (`AppConfig`).
//!
//! Includes defensive safeguards:
//! - `#[serde(default)]` on all structs and fields so missing or newly added keys never fail to load.
//! - `AppConfig::sanitize()` to clamp hand-edited out-of-range values and normalize language codes.
//! - `AppConfig::save_to_file()` using atomic `.tmp` write + rename so power loss never corrupts `config.json`.
//! - `AppConfig::redacted_for_sync()` to strip secret tokens (`github_personal_access_token`) before export/sync.

use crate::{IkazuchiError, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Theme mode matching Mihon's `ThemeMode` (`UiPreferences.kt`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ThemeMode {
    #[default]
    Dark,
    Light,
    System,
}

/// Color palette matching Mihon's `AppTheme` (`UiPreferences.kt`) adapted for Ikazuchi.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AppTheme {
    #[default]
    IkazuchiDefault,
    RaijinIndigo,
    CrimsonStorm,
    Monochrome,
}

/// UI & Appearance preferences (`UiPreferences.kt`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiSettings {
    pub theme_mode: ThemeMode,
    pub app_theme: AppTheme,
    /// Pure `#000000` background in dark mode for OLED screens (`pref_theme_dark_amoled_key`).
    pub theme_dark_amoled: bool,
    /// Show relative timestamps (e.g. `"2 days ago"`) for chapter uploads (`relative_time`).
    pub relative_timestamps: bool,
    /// Date format string (`date_format`, default `"YYYY-MM-DD"`).
    pub date_format: String,
    /// Render inline images in manga descriptions (`images_in_description`).
    pub images_in_description: bool,
}

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            theme_mode: ThemeMode::Dark,
            app_theme: AppTheme::IkazuchiDefault,
            theme_dark_amoled: false,
            relative_timestamps: true,
            date_format: "YYYY-MM-DD".to_string(),
            images_in_description: true,
        }
    }
}

/// Library grid/list display mode (`LibraryDisplayMode` in `LibraryPreferences.kt`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LibraryDisplayMode {
    #[default]
    CompactGrid,
    ComfortableGrid,
    CoverOnlyGrid,
    List,
}

/// Library sorting criteria (`LibrarySort` in `LibraryPreferences.kt` + Yokai `Random`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LibrarySortMode {
    #[default]
    Alphabetical,
    LastRead,
    LastUpdate,
    UnreadCount,
    TotalChapters,
    DateAdded,
    Random,
}

/// Dynamic library grouping mode adapted from Yokai / J2K (`LibraryGroup.kt`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LibraryGroupMode {
    #[default]
    ByCategory,
    ByStatus,
    BySource,
    ByGenre,
    ByAuthor,
    Ungrouped,
}

/// Library display, badges, dynamic grouping, and scheduled update settings (`LibraryPreferences.kt`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LibrarySettings {
    pub display_mode: LibraryDisplayMode,
    pub sort_mode: LibrarySortMode,
    pub sort_ascending: bool,
    pub group_mode: LibraryGroupMode,
    /// Portrait grid columns (`0` = automatic responsive fit, `1..=12`).
    pub portrait_columns: u32,
    /// Landscape/desktop grid columns (`0` = automatic responsive fit, `1..=16`).
    pub landscape_columns: u32,
    /// Scheduled background library update interval in hours (`0` = Off, `6`, `12`, `24`, `48`, `168`).
    pub auto_update_interval_hours: u32,
    /// Only update `Ongoing` mangas during scheduled updates (`"manga_ongoing"` restriction).
    pub update_only_ongoing: bool,
    /// Skip mangas that already have unread chapters (`"manga_fully_read"` restriction).
    pub skip_unread_entries: bool,
    /// Skip mangas with zero read chapters (`"manga_started"` restriction).
    pub skip_unstarted_entries: bool,
    /// Skip mangas before their predicted next release window (`"manga_outside_release_period"`).
    pub skip_outside_release_period: bool,
    /// Automatically refresh cover/synopsis metadata during library updates (`auto_update_metadata`).
    pub auto_refresh_metadata: bool,
    /// If non-empty, only mangas in at least one of these categories will be updated.
    pub included_categories: Vec<String>,
    /// Mangas in any of these categories will always be skipped during updates.
    pub excluded_categories: Vec<String>,
    pub show_unread_badge: bool,
    pub show_download_badge: bool,
    pub show_local_badge: bool,
    pub show_language_badge: bool,
    pub show_continue_reading_button: bool,
}

impl Default for LibrarySettings {
    fn default() -> Self {
        Self {
            display_mode: LibraryDisplayMode::CompactGrid,
            sort_mode: LibrarySortMode::Alphabetical,
            sort_ascending: true,
            group_mode: LibraryGroupMode::ByCategory,
            portrait_columns: 0,
            landscape_columns: 0,
            auto_update_interval_hours: 12,
            update_only_ongoing: true,
            skip_unread_entries: false,
            skip_unstarted_entries: false,
            skip_outside_release_period: true,
            auto_refresh_metadata: false,
            included_categories: Vec::new(),
            excluded_categories: Vec::new(),
            show_unread_badge: true,
            show_download_badge: true,
            show_local_badge: true,
            show_language_badge: false,
            show_continue_reading_button: true,
        }
    }
}

/// Default reading mode matching Mihon's `ReadingMode` (`ReaderPreferences.kt`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ReadingModeSetting {
    #[default]
    WebtoonContinuous,
    PagedRtl,
    PagedLtr,
}

/// Image scaling mode in the reader (`ReaderPreferences.imageScaleType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ImageScaleType {
    #[default]
    FitWidth,
    FitScreen,
    FitHeight,
    OriginalSize,
    SmartFit,
}

/// Reader defaults and image processing preferences (`ReaderPreferences.kt`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReaderSettings {
    pub default_reading_mode: ReadingModeSetting,
    /// Default viewport zoom width percentage (`30..=100`, default `65`).
    pub default_zoom_width_pct: u32,
    pub image_scale_type: ImageScaleType,
    pub keep_screen_on: bool,
    pub fullscreen: bool,
    pub show_page_number: bool,
    pub crop_borders: bool,
    pub crop_borders_webtoon: bool,
    /// Horizontal side padding percentage in Webtoon mode (`0..=25`, matching Mihon's `webtoonSidePadding`).
    pub webtoon_side_padding_pct: u32,
    pub dual_page_split: bool,
    pub grayscale: bool,
    pub inverted_colors: bool,
    pub skip_read: bool,
    pub skip_filtered: bool,
    pub skip_duplicate: bool,
    /// Automatically mark duplicate chapters (same chapter number across scanlators) as read (Yokai 1.10.0).
    pub auto_mark_duplicate_chapters_read: bool,
}

impl Default for ReaderSettings {
    fn default() -> Self {
        Self {
            default_reading_mode: ReadingModeSetting::WebtoonContinuous,
            default_zoom_width_pct: 65,
            image_scale_type: ImageScaleType::FitWidth,
            keep_screen_on: true,
            fullscreen: true,
            show_page_number: true,
            crop_borders: false,
            crop_borders_webtoon: false,
            webtoon_side_padding_pct: 0,
            dual_page_split: false,
            grayscale: false,
            inverted_colors: false,
            skip_read: false,
            skip_filtered: true,
            skip_duplicate: false,
            auto_mark_duplicate_chapters_read: false,
        }
    }
}

/// Extension & Catalog filtering settings (`SourcePreferences.kt`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExtensionSettings {
    /// Whether to display `18+` / `NSFW` (`nsfw == 1` / `CONTENT_WARNING_NSFW`) extensions in the catalog.
    /// Defaults to `false` (hidden) for safety.
    pub show_nsfw: bool,
    /// Whether to also hide already-installed NSFW extensions/sources when `show_nsfw == false`
    /// (matches Mihon's `applyContentWarningsToInstalled()`).
    pub apply_nsfw_filter_to_installed: bool,
    /// Enabled ISO language codes (e.g. `["en", "all"]`). Empty list means all languages are allowed.
    pub enabled_languages: Vec<String>,
    /// Explicitly disabled/hidden Mihon 63-bit source IDs (`"hidden_catalogues"`).
    pub disabled_source_ids: Vec<i64>,
    /// Pinned Mihon 63-bit source IDs (`"pinned_catalogues"`), sorted first in Horizon & Global Search.
    pub pinned_source_ids: Vec<i64>,
    /// Hide mangas already in the user's Sanctuary when browsing a catalog (`"browse_hide_in_library_items"`).
    pub hide_in_library_items: bool,
    /// Configured Mihon / Keiyoushi extension repository URLs (`index.json` / `index.min.json`).
    pub extension_repos: Vec<String>,
}

impl Default for ExtensionSettings {
    fn default() -> Self {
        Self {
            show_nsfw: false,
            apply_nsfw_filter_to_installed: true,
            enabled_languages: vec!["en".to_string(), "all".to_string()],
            disabled_source_ids: Vec::new(),
            pinned_source_ids: Vec::new(),
            hide_in_library_items: false,
            extension_repos: vec![
                "https://raw.githubusercontent.com/keiyoushi/extensions/repo/index.json".to_string(),
            ],
        }
    }
}

impl ExtensionSettings {
    /// Returns `true` if a language code (e.g. `"en"`, `"ja"`, `"all"`) is enabled in settings.
    pub fn is_language_enabled(&self, lang: &str) -> bool {
        if self.enabled_languages.is_empty() {
            return true;
        }
        let norm = lang.trim().to_ascii_lowercase();
        self.enabled_languages
            .iter()
            .any(|l| l.eq_ignore_ascii_case(&norm))
    }

    /// Returns `true` if a source ID is explicitly disabled by the user.
    pub fn is_source_disabled(&self, source_id: i64) -> bool {
        self.disabled_source_ids.contains(&source_id)
    }

    /// Returns `true` if a source ID is pinned by the user.
    pub fn is_source_pinned(&self, source_id: i64) -> bool {
        self.pinned_source_ids.contains(&source_id)
    }
}

/// Offline `.cbz` Download preferences (`DownloadPreferences.kt`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DownloadSettings {
    pub download_only_over_wifi: bool,
    pub save_chapters_as_cbz: bool,
    /// Number of upcoming chapters to auto-download while reading (`0` = Off, `2`, `3`, `5`, `10`).
    pub auto_download_while_reading: u32,
    pub remove_after_marked_as_read: bool,
    /// Delete read chapter after reading N chapters ahead (`-1` = Disabled, `0` = Last read, `1..=4`).
    pub remove_after_read_slots: i32,
    pub remove_bookmarked_chapters: bool,
    /// Automatically download newly discovered chapters during library updates.
    pub download_new_chapters: bool,
    /// Max sources to download from concurrently (`1..=10`, default `5`).
    pub parallel_source_limit: usize,
    /// Max pages per chapter to download concurrently (`1..=15`, default `5`).
    pub parallel_page_limit: usize,
}

impl Default for DownloadSettings {
    fn default() -> Self {
        Self {
            download_only_over_wifi: true,
            save_chapters_as_cbz: true,
            auto_download_while_reading: 0,
            remove_after_marked_as_read: false,
            remove_after_read_slots: -1,
            remove_bookmarked_chapters: false,
            download_new_chapters: false,
            parallel_source_limit: 5,
            parallel_page_limit: 5,
        }
    }
}

/// Backup & Serverless GitHub Sync settings (`BackupPreferences.kt` + Ikazuchi Vault).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BackupSyncSettings {
    /// Automatic local `.tachibk` backup interval in hours (`0` = Manual only, default `12`).
    pub backup_interval_hours: u32,
    /// Maximum `.tachibk` backup files to retain on disk (`1..=30`, default `5`).
    pub max_backup_files: u32,
    pub include_settings_in_backup: bool,
    pub sync_settings_to_github: bool,
    pub github_owner: String,
    pub github_repo: String,
    pub github_branch: String,
    pub github_personal_access_token: String,
    pub github_api_base_url: String,
}

impl Default for BackupSyncSettings {
    fn default() -> Self {
        Self {
            backup_interval_hours: 12,
            max_backup_files: 5,
            include_settings_in_backup: true,
            sync_settings_to_github: true,
            github_owner: "notsoeazy".to_string(),
            github_repo: "ikazuchi-vault-sync".to_string(),
            github_branch: "main".to_string(),
            github_personal_access_token: String::new(),
            github_api_base_url: "https://api.github.com".to_string(),
        }
    }
}

/// Root application configuration persisted to `config.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub version: u32,
    pub ui: UiSettings,
    pub library: LibrarySettings,
    pub reader: ReaderSettings,
    pub extensions: ExtensionSettings,
    pub downloads: DownloadSettings,
    pub backup_sync: BackupSyncSettings,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            version: 1,
            ui: UiSettings::default(),
            library: LibrarySettings::default(),
            reader: ReaderSettings::default(),
            extensions: ExtensionSettings::default(),
            downloads: DownloadSettings::default(),
            backup_sync: BackupSyncSettings::default(),
        }
    }
}

impl AppConfig {
    /// Clamps out-of-range numeric values, deduplicates lists, and normalizes language codes & URLs
    /// to protect against human error when hand-editing `config.json`.
    pub fn sanitize(&mut self) {
        if self.version == 0 {
            self.version = 1;
        }
        if self.ui.date_format.trim().is_empty() {
            self.ui.date_format = "YYYY-MM-DD".to_string();
        }

        // Library bounds
        self.library.portrait_columns = self.library.portrait_columns.min(12);
        self.library.landscape_columns = self.library.landscape_columns.min(16);
        self.library.auto_update_interval_hours = self.library.auto_update_interval_hours.min(168);

        // Reader bounds
        self.reader.default_zoom_width_pct = self.reader.default_zoom_width_pct.clamp(30, 100);
        self.reader.webtoon_side_padding_pct = self.reader.webtoon_side_padding_pct.min(25);

        // Extension language & repo normalization
        let mut norm_langs = Vec::with_capacity(self.extensions.enabled_languages.len());
        for lang in &self.extensions.enabled_languages {
            let trimmed = lang.trim().to_ascii_lowercase();
            if !trimmed.is_empty() && !norm_langs.contains(&trimmed) {
                norm_langs.push(trimmed);
            }
        }
        self.extensions.enabled_languages = norm_langs;
        self.extensions.disabled_source_ids.dedup();
        self.extensions.pinned_source_ids.dedup();

        for repo in &mut self.extensions.extension_repos {
            let trimmed = repo.trim();
            if trimmed.contains("keiyoushi/extensions") && trimmed.ends_with("/index.min.json") {
                *repo = trimmed.replace("/index.min.json", "/index.json");
            } else {
                *repo = trimmed.to_string();
            }
        }
        self.extensions.extension_repos.retain(|r| !r.is_empty());
        self.extensions.extension_repos.dedup();

        // Download bounds (prevent 0-permit semaphores!)
        self.downloads.auto_download_while_reading =
            self.downloads.auto_download_while_reading.min(10);
        self.downloads.remove_after_read_slots =
            self.downloads.remove_after_read_slots.clamp(-1, 5);
        self.downloads.parallel_source_limit = self.downloads.parallel_source_limit.clamp(1, 10);
        self.downloads.parallel_page_limit = self.downloads.parallel_page_limit.clamp(1, 15);

        // Backup & Sync bounds
        self.backup_sync.max_backup_files = self.backup_sync.max_backup_files.clamp(1, 30);
        if self.backup_sync.github_branch.trim().is_empty() {
            self.backup_sync.github_branch = "main".to_string();
        }
        if self.backup_sync.github_api_base_url.trim().is_empty() {
            self.backup_sync.github_api_base_url = "https://api.github.com".to_string();
        }
    }

    /// Parses an `AppConfig` from a JSON string and automatically runs [`Self::sanitize`].
    pub fn from_json_str(json: &str) -> Result<Self> {
        let mut cfg: Self =
            serde_json::from_str(json).map_err(|e| IkazuchiError::Parse(e.to_string()))?;
        cfg.sanitize();
        Ok(cfg)
    }

    /// Serializes this `AppConfig` to pretty-printed JSON after running [`Self::sanitize`].
    pub fn to_json_pretty(&self) -> Result<String> {
        let mut sanitized = self.clone();
        sanitized.sanitize();
        serde_json::to_string_pretty(&sanitized).map_err(|e| IkazuchiError::Parse(e.to_string()))
    }

    /// Loads `AppConfig` from `path`. If the file does not exist yet, returns `AppConfig::default()`.
    pub fn load_from_file(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = std::fs::read_to_string(path)
            .map_err(|e| IkazuchiError::Database(format!("Failed to read config {}: {e}", path.display())))?;
        if raw.trim().is_empty() {
            return Ok(Self::default());
        }
        Self::from_json_str(&raw)
    }

    /// Atomically persists `AppConfig` to `path` by writing to `<path>.tmp` and renaming it over `path`.
    pub fn save_to_file(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    IkazuchiError::Database(format!(
                        "Failed to create config directory {}: {e}",
                        parent.display()
                    ))
                })?;
            }
        }
        let json = self.to_json_pretty()?;
        let tmp_path = path.with_extension("json.tmp");
        std::fs::write(&tmp_path, json.as_bytes()).map_err(|e| {
            IkazuchiError::Database(format!("Failed to write temp config {}: {e}", tmp_path.display()))
        })?;
        std::fs::rename(&tmp_path, path).map_err(|e| {
            let _ = std::fs::remove_file(&tmp_path);
            IkazuchiError::Database(format!("Failed to commit config {}: {e}", path.display()))
        })?;
        Ok(())
    }

    /// Returns a sanitized copy with `github_personal_access_token` redacted so secrets are never
    /// committed to GitHub (`settings/config.json`) or leaked in exported backups.
    pub fn redacted_for_sync(&self) -> Self {
        let mut copy = self.clone();
        copy.sanitize();
        copy.backup_sync.github_personal_access_token.clear();
        copy
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_atomic_save_load_and_missing_file_fallback() {
        let dir = std::env::temp_dir().join("ikazuchi_config_test");
        let _ = std::fs::remove_dir_all(&dir);
        let cfg_path = dir.join("config.json");

        // 1. Missing file returns clean Default
        let mut cfg = AppConfig::load_from_file(&cfg_path).unwrap();
        assert!(!cfg.extensions.show_nsfw);
        assert_eq!(cfg.library.auto_update_interval_hours, 12);

        // 2. Modify settings and save atomically
        cfg.ui.theme_dark_amoled = true;
        cfg.extensions.show_nsfw = true;
        cfg.extensions.enabled_languages = vec!["EN".into(), "ja".into()];
        cfg.reader.default_reading_mode = ReadingModeSetting::PagedRtl;
        cfg.save_to_file(&cfg_path).unwrap();

        assert!(cfg_path.exists());
        assert!(!cfg_path.with_extension("json.tmp").exists());

        // 3. Reload from disk and verify normalization
        let loaded = AppConfig::load_from_file(&cfg_path).unwrap();
        assert!(loaded.ui.theme_dark_amoled);
        assert!(loaded.extensions.show_nsfw);
        assert_eq!(loaded.extensions.enabled_languages, vec!["en", "ja"]);
        assert_eq!(loaded.reader.default_reading_mode, ReadingModeSetting::PagedRtl);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_config_sanitize_clamps_hand_edited_out_of_bounds_values() {
        let hand_edited_json = r#"{
            "reader": {
                "default_zoom_width_pct": 999,
                "webtoon_side_padding_pct": 85
            },
            "downloads": {
                "parallel_source_limit": 0,
                "parallel_page_limit": 500,
                "remove_after_read_slots": -99
            },
            "extensions": {
                "enabled_languages": [" EN ", "en", "JA", ""],
                "extension_repos": [
                    "https://raw.githubusercontent.com/keiyoushi/extensions/repo/index.min.json"
                ]
            },
            "backup_sync": {
                "github_personal_access_token": "ghp_secret123456"
            }
        }"#;

        let cfg = AppConfig::from_json_str(hand_edited_json).unwrap();
        assert_eq!(cfg.reader.default_zoom_width_pct, 100);
        assert_eq!(cfg.reader.webtoon_side_padding_pct, 25);
        assert_eq!(cfg.downloads.parallel_source_limit, 1);
        assert_eq!(cfg.downloads.parallel_page_limit, 15);
        assert_eq!(cfg.downloads.remove_after_read_slots, -1);
        assert_eq!(cfg.extensions.enabled_languages, vec!["en", "ja"]);
        assert_eq!(
            cfg.extensions.extension_repos,
            vec!["https://raw.githubusercontent.com/keiyoushi/extensions/repo/index.json"]
        );

        // Verify secret token redaction for sync/backup
        let redacted = cfg.redacted_for_sync();
        assert!(redacted.backup_sync.github_personal_access_token.is_empty());
    }
}
