//! # Extension Manager & Automatic Source Instantiation (`ExtensionManager`)
//!
//! Manages the lifecycle of installed Mihon extensions:
//! 1. Unpacks an `.apk` archive and parses `AndroidManifest.xml` (`axml_parser`) + `classes.dex` (`dex_parser`).
//! 2. Inspects the DEX class hierarchy (`DexClassDef.superclass_descriptor`) to detect the Multisrc theme.
//! 3. Automatically instantiates the corresponding native Rust `Arc<dyn MangaSource>` driver
//!    (`MadaraSource`, `MangaThemesiaSource`, `MangaboxSource`, `MangaDexSource`).

use crate::axml_parser::{ExtensionManifest, parse_android_manifest};
use crate::dex_parser::DexFile;
use crate::drivers::{
    local::LOCAL_SOURCE_ID, madara::MadaraSource, mangabox::MangaboxSource,
    mangadex::MangaDexSource, mangathemesia::MangaThemesiaSource,
};
use crate::{InspectedApkMetadata, MultisrcTheme, RepoExtensionEntry};
use ikazuchi_core::{ExtensionSettings, FilterList, IkazuchiError, MangaSource, Result, SManga};
use std::collections::{HashMap, HashSet};
use std::io::{Cursor, Read};
use std::sync::Arc;
use std::time::Duration;
use zip::ZipArchive;

/// Result from a single source during a concurrent multi-source global search.
#[derive(Debug, Clone)]
pub struct GlobalSearchResult {
    pub source_id: i64,
    pub source_name: String,
    pub source_lang: String,
    pub mangas: Vec<SManga>,
    pub error: Option<String>,
}

/// A loaded extension package and its instantiated `MangaSource` providers.
pub struct LoadedExtension {
    pub pkg_name: String,
    pub version_name: String,
    pub version_code: i64,
    pub nsfw: bool,
    pub theme: MultisrcTheme,
    pub sources: Vec<Arc<dyn MangaSource>>,
}

/// Registry and loader for Mihon-compatible extensions in Ikazuchi.
#[derive(Default)]
pub struct ExtensionManager {
    extensions: HashMap<String, LoadedExtension>,
    sources_by_id: HashMap<i64, Arc<dyn MangaSource>>,
    nsfw_source_ids: HashSet<i64>,
}

impl ExtensionManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a built-in or manually constructed `MangaSource`.
    pub fn register_source(&mut self, source: Arc<dyn MangaSource>) {
        self.register_source_with_nsfw(source, false);
    }

    /// Registers a `MangaSource` and marks whether it contains `18+` / `NSFW` content.
    pub fn register_source_with_nsfw(&mut self, source: Arc<dyn MangaSource>, nsfw: bool) {
        let id = source.id();
        if nsfw {
            self.nsfw_source_ids.insert(id);
        } else {
            self.nsfw_source_ids.remove(&id);
        }
        self.sources_by_id.insert(id, source);
    }

    /// Returns `true` if the installed source with `source_id` is marked as `18+` / `NSFW`.
    pub fn is_source_nsfw(&self, source_id: i64) -> bool {
        self.nsfw_source_ids.contains(&source_id)
    }

    /// Retrieves a registered `MangaSource` by its deterministic 63-bit Mihon ID.
    pub fn get_source(&self, source_id: i64) -> Option<Arc<dyn MangaSource>> {
        self.sources_by_id.get(&source_id).cloned()
    }

    /// Lists all active `MangaSource` instances across all loaded extensions.
    pub fn all_sources(&self) -> Vec<Arc<dyn MangaSource>> {
        self.sources_by_id.values().cloned().collect()
    }

    /// Lists installed `MangaSource` instances filtered and sorted by the user's [`ExtensionSettings`]
    /// (hiding NSFW sources when `!show_nsfw && apply_nsfw_filter_to_installed`, filtering by
    /// `enabled_languages` and `disabled_source_ids`, and sorting `pinned_source_ids` first).
    pub fn visible_sources(&self, settings: &ExtensionSettings) -> Vec<Arc<dyn MangaSource>> {
        let mut visible: Vec<Arc<dyn MangaSource>> = self
            .sources_by_id
            .values()
            .filter(|src| {
                let id = src.id();
                if settings.is_source_disabled(id) {
                    return false;
                }
                if !settings.show_nsfw
                    && settings.apply_nsfw_filter_to_installed
                    && self.is_source_nsfw(id)
                {
                    return false;
                }
                if id == LOCAL_SOURCE_ID {
                    return true;
                }
                settings.is_language_enabled(src.lang())
            })
            .cloned()
            .collect();

        visible.sort_by(|a, b| {
            let a_pinned = settings.is_source_pinned(a.id());
            let b_pinned = settings.is_source_pinned(b.id());
            b_pinned
                .cmp(&a_pinned)
                .then_with(|| a.name().cmp(b.name()))
                .then_with(|| a.id().cmp(&b.id()))
        });
        visible
    }

    /// Lists all loaded extension packages.
    pub fn loaded_extensions(&self) -> impl Iterator<Item = &LoadedExtension> {
        self.extensions.values()
    }

    /// Executes a concurrent multi-source search across all registered `MangaSource` instances,
    /// isolating each source with `timeout_per_source` so a slow or offline site never blocks other sources.
    pub async fn global_search(
        &self,
        query: &str,
        timeout_per_source: Duration,
    ) -> Vec<GlobalSearchResult> {
        let mut sources = self.all_sources();
        sources.sort_by(|a, b| a.name().cmp(b.name()).then_with(|| a.id().cmp(&b.id())));
        Self::run_concurrent_search(sources, query, timeout_per_source).await
    }

    /// Executes a concurrent multi-source search across only the sources visible under `settings`
    /// (excluding NSFW, disabled, and non-enabled-language sources, with pinned sources first).
    pub async fn global_search_filtered(
        &self,
        query: &str,
        settings: &ExtensionSettings,
        timeout_per_source: Duration,
    ) -> Vec<GlobalSearchResult> {
        let sources = self.visible_sources(settings);
        Self::run_concurrent_search(sources, query, timeout_per_source).await
    }

    async fn run_concurrent_search(
        sources: Vec<Arc<dyn MangaSource>>,
        query: &str,
        timeout_per_source: Duration,
    ) -> Vec<GlobalSearchResult> {
        let trimmed = query.trim();
        if trimmed.is_empty() {
            return Vec::new();
        }

        let mut handles = Vec::with_capacity(sources.len());
        for src in sources {
            let q = trimmed.to_string();
            handles.push(tokio::spawn(async move {
                let filters: FilterList = Vec::new();
                match tokio::time::timeout(
                    timeout_per_source,
                    src.get_search_manga(1, &q, &filters),
                )
                .await
                {
                    Ok(Ok(page)) => GlobalSearchResult {
                        source_id: src.id(),
                        source_name: src.name().to_string(),
                        source_lang: src.lang().to_string(),
                        mangas: page.mangas,
                        error: None,
                    },
                    Ok(Err(e)) => GlobalSearchResult {
                        source_id: src.id(),
                        source_name: src.name().to_string(),
                        source_lang: src.lang().to_string(),
                        mangas: Vec::new(),
                        error: Some(e.to_string()),
                    },
                    Err(_) => GlobalSearchResult {
                        source_id: src.id(),
                        source_name: src.name().to_string(),
                        source_lang: src.lang().to_string(),
                        mangas: Vec::new(),
                        error: Some(format!("Timed out after {:?}", timeout_per_source)),
                    },
                }
            }));
        }

        let mut results = Vec::with_capacity(handles.len());
        for h in handles {
            if let Ok(res) = h.await {
                results.push(res);
            }
        }
        results
    }

    /// Installs and loads a Mihon `.apk` package from raw bytes, using optional repository index
    /// metadata (`RepoExtensionEntry`) to supplement multi-source names/IDs.
    pub fn load_apk_from_bytes(
        &mut self,
        apk_bytes: &[u8],
        repo_entry: Option<&RepoExtensionEntry>,
    ) -> Result<&LoadedExtension> {
        let (manifest_opt, dex_file, inspected) = unpack_and_analyze_apk(apk_bytes)?;

        let pkg_name = repo_entry
            .map(|r| r.pkg.clone())
            .or_else(|| manifest_opt.as_ref().map(|m| m.package_name.clone()))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "eu.kanade.tachiyomi.extension.unknown".to_string());

        let version_name = repo_entry
            .map(|r| r.version.clone())
            .unwrap_or_else(|| "1.4.1".to_string());
        let version_code = repo_entry.map(|r| r.code).unwrap_or(1);
        let nsfw = repo_entry
            .map(|r| r.nsfw == 1)
            .or_else(|| manifest_opt.as_ref().map(|m| m.nsfw))
            .unwrap_or(false);

        let fallback_url = inspected
            .extracted_urls
            .first()
            .cloned()
            .unwrap_or_else(|| "https://example.org".to_string());

        let mut instantiated_sources: Vec<Arc<dyn MangaSource>> = Vec::new();

        if let Some(entry) = repo_entry.filter(|e| !e.sources.is_empty()) {
            for src_meta in &entry.sources {
                let base_url = if src_meta.base_url.is_empty() {
                    fallback_url.clone()
                } else {
                    src_meta.base_url.clone()
                };
                if let Some(driver) = instantiate_driver(
                    &inspected.detected_theme,
                    &src_meta.name,
                    &base_url,
                    &src_meta.lang,
                ) {
                    self.register_source_with_nsfw(driver.clone(), nsfw);
                    instantiated_sources.push(driver);
                }
            }
        } else {
            // Derive name and lang directly from DEX strings / package name
            let lang = pkg_name.split('.').nth(4).unwrap_or("en").to_string();
            let raw_name = dex_file
                .classes
                .first()
                .and_then(|c| c.source_file.as_deref())
                .map(|sf| sf.trim_end_matches(".kt").trim_end_matches(".java").to_string())
                .unwrap_or_else(|| pkg_name.split('.').next_back().unwrap_or("Source").to_string());

            if let Some(driver) = instantiate_driver(&inspected.detected_theme, &raw_name, &fallback_url, &lang) {
                self.register_source_with_nsfw(driver.clone(), nsfw);
                instantiated_sources.push(driver);
            }
        }

        let loaded = LoadedExtension {
            pkg_name: pkg_name.clone(),
            version_name,
            version_code,
            nsfw,
            theme: inspected.detected_theme,
            sources: instantiated_sources,
        };

        self.extensions.insert(pkg_name.clone(), loaded);
        Ok(self.extensions.get(&pkg_name).unwrap())
    }
}

fn instantiate_driver(
    theme: &MultisrcTheme,
    name: &str,
    base_url: &str,
    lang: &str,
) -> Option<Arc<dyn MangaSource>> {
    match theme {
        MultisrcTheme::Madara => Some(Arc::new(MadaraSource::new(name, base_url, lang, 1))),
        MultisrcTheme::MangaThemesia | MultisrcTheme::MangaStream => {
            Some(Arc::new(MangaThemesiaSource::new(name, base_url, lang, 1)))
        }
        MultisrcTheme::Mangabox => Some(Arc::new(MangaboxSource::new(name, base_url, lang, 1))),
        MultisrcTheme::MangaDex => Some(Arc::new(MangaDexSource::new(lang))),
        _ => {
            // Default to Madara/generic HTTP scraper or stage-3 bridge
            Some(Arc::new(MadaraSource::new(name, base_url, lang, 1)))
        }
    }
}

/// Unpacks `AndroidManifest.xml` and `classes.dex` from an `.apk` buffer and analyzes the DEX class hierarchy.
pub fn unpack_and_analyze_apk(apk_bytes: &[u8]) -> Result<(Option<ExtensionManifest>, DexFile, InspectedApkMetadata)> {
    let reader = Cursor::new(apk_bytes);
    let mut archive = ZipArchive::new(reader).map_err(|e| IkazuchiError::Parse(e.to_string()))?;

    let manifest_opt = if let Ok(mut mf) = archive.by_name("AndroidManifest.xml") {
        let mut mf_bytes = Vec::new();
        let _ = mf.read_to_end(&mut mf_bytes);
        parse_android_manifest(&mf_bytes).ok()
    } else {
        None
    };

    let mut dex_entry = archive
        .by_name("classes.dex")
        .map_err(|e| IkazuchiError::Parse(format!("Missing classes.dex in APK: {e}")))?;
    let mut dex_bytes = Vec::new();
    dex_entry
        .read_to_end(&mut dex_bytes)
        .map_err(|e| IkazuchiError::Parse(e.to_string()))?;

    let dex_file = DexFile::parse(&dex_bytes)?;
    let detected_theme = detect_theme_from_dex(&dex_file);
    let extracted_urls = dex_file.extract_http_urls();

    let inspected = InspectedApkMetadata {
        detected_theme,
        extracted_urls,
        dex_size_bytes: dex_bytes.len(),
    };

    Ok((manifest_opt, dex_file, inspected))
}

/// Inspects the structured `class_defs` superclass descriptors and `types` table inside a parsed `DexFile`
/// to identify the exact Mihon Multisrc theme.
pub fn detect_theme_from_dex(dex: &DexFile) -> MultisrcTheme {
    // 1. Check structured superclass descriptors in `class_defs` first
    for cls in &dex.classes {
        if let Some(sup) = &cls.superclass_descriptor {
            if sup.contains("multisrc/madara/Madara") {
                return MultisrcTheme::Madara;
            } else if sup.contains("multisrc/mangathemesia/MangaThemesia") {
                return MultisrcTheme::MangaThemesia;
            } else if sup.contains("multisrc/mangastream/MangaStream") {
                return MultisrcTheme::MangaStream;
            } else if sup.contains("multisrc/mangabox/Mangabox") {
                return MultisrcTheme::Mangabox;
            } else if sup.contains("multisrc/keyoapp/Keyoapp") || sup.contains("multisrc/heancms") {
                return MultisrcTheme::Keyoapp;
            } else if sup.contains("multisrc/zeistmanga/ZeistManga") {
                return MultisrcTheme::ZeistManga;
            }
        }
    }

    // 2. Check string/type tables
    for s in &dex.strings {
        if s.contains("api.mangadex.org") {
            return MultisrcTheme::MangaDex;
        } else if s.contains("multisrc/madara/Madara") {
            return MultisrcTheme::Madara;
        } else if s.contains("multisrc/mangathemesia/MangaThemesia") {
            return MultisrcTheme::MangaThemesia;
        }
    }

    MultisrcTheme::CustomBytecode
}
