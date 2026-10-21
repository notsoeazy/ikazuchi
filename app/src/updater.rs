//! # Scheduled & Manual Library Update Engine (`LibraryUpdater`)
//!
//! Ports Mihon's `LibraryUpdateWorker.kt` to Rust/Tokio with:
//! - **Manual Update Trigger:** Update the entire library, a single category, or a single manga on demand.
//! - **Scheduled Periodic Updater:** Background interval timer that automatically polls for new chapter drops.
//! - **Safeguard #12 (Anti-Ban Source-Grouped Concurrency & Single-Flight Lock):**
//!   - Uses an `AtomicBool` lock so a scheduled update and a manual update never run concurrently.
//!   - Evaluates Mihon's smart skip rules (`LibraryUpdateConfig`: `Completed`, `OnlyFetchOnce`, `HasUnread`,
//!     `NotStarted`, `ExcludedCategory`, `OutsideReleasePeriod`).
//!   - Groups eligible mangas by `source_id`, polling up to `max_parallel_sources` (default `5`) distinct
//!     sources concurrently while fetching mangas within the *same* source sequentially.

use ikazuchi_core::SChapter;
use ikazuchi_db::{
    ChapterRecord, Database, LibraryMangaItem, LibraryUpdateConfig, UpdateSkipReason,
};
use ikazuchi_dex::ExtensionManager;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::Semaphore;

/// Summary report produced by a manual or scheduled library update run.
#[derive(Debug, Clone, Default)]
pub struct LibraryUpdateSummary {
    /// True if the update request was ignored because another update was already in flight.
    pub was_already_running: bool,
    /// Number of mangas successfully polled.
    pub mangas_checked: usize,
    /// Newly discovered visible chapters per `(manga_id, manga_title, Vec<ChapterRecord>)`.
    pub new_drops: Vec<(i64, String, Vec<ChapterRecord>)>,
    /// Mangas skipped due to `LibraryUpdateConfig` restrictions.
    pub skipped: Vec<(i64, String, UpdateSkipReason)>,
    /// Mangas that failed to update `(manga_id, manga_title, error_message)`.
    pub failed: Vec<(i64, String, String)>,
}

impl LibraryUpdateSummary {
    pub fn total_new_chapters(&self) -> usize {
        self.new_drops.iter().map(|(_, _, chs)| chs.len()).sum()
    }
}

/// Coordinates manual and scheduled library updates with single-flight locking
/// and source-grouped concurrency.
#[derive(Clone, Default)]
pub struct LibraryUpdater {
    is_running: Arc<AtomicBool>,
}

struct RunningGuard(Arc<AtomicBool>);

impl Drop for RunningGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

impl LibraryUpdater {
    pub fn new() -> Self {
        Self {
            is_running: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Returns `true` if a manual or scheduled update is currently in progress.
    pub fn is_updating(&self) -> bool {
        self.is_running.load(Ordering::SeqCst)
    }

    /// Returns a clone of the internal atomic running flag (for inspection and tests).
    pub fn is_running_flag(&self) -> Arc<AtomicBool> {
        self.is_running.clone()
    }

    /// Manually updates a single manga by `manga_id` (bypassing category/status skip filters).
    pub async fn update_single_manga(
        &self,
        db: &Database,
        ext_mgr: &ExtensionManager,
        manga_id: i64,
        fetch_metadata: bool,
        now_ms: i64,
    ) -> Result<Vec<ChapterRecord>, String> {
        let item = db
            .get_manga_by_id(manga_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("Manga {manga_id} not found"))?;

        let source = ext_mgr
            .get_source(item.source_id)
            .ok_or_else(|| format!("Source {} is not installed", item.source_id))?;

        if fetch_metadata {
            if let Ok(updated_meta) = source.get_manga_details(&item.manga).await {
                let fav_opt = if item.favorite_at > 0 {
                    Some(item.favorite_at)
                } else {
                    None
                };
                let _ = db.upsert_manga(item.source_id, &updated_meta, fav_opt, now_ms);
            }
        }

        let remote_chapters = source
            .get_chapter_list(&item.manga)
            .await
            .map_err(|e| e.to_string())?;

        db.sync_chapters_detecting_new(manga_id, &remote_chapters, now_ms)
            .map_err(|e| e.to_string())
    }

    /// Runs a full library (or single-category) update pass.
    /// Can be called manually by the user or triggered by the scheduled background timer.
    pub async fn run_update(
        &self,
        db: &Database,
        ext_mgr: &ExtensionManager,
        config: &LibraryUpdateConfig,
        target_category: Option<&str>,
        now_ms: i64,
    ) -> LibraryUpdateSummary {
        // Safeguard #12a: Single-flight atomic lock with RAII drop guard prevents overlapping runs
        // and guarantees lock release even if the task is cancelled or panics.
        if self
            .is_running
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return LibraryUpdateSummary {
                was_already_running: true,
                ..Default::default()
            };
        }
        let _guard = RunningGuard(self.is_running.clone());

        let (eligible, skipped) = match db.filter_library_for_update(config, target_category, now_ms) {
            Ok(pair) => pair,
            Err(e) => {
                return LibraryUpdateSummary {
                    failed: vec![(0, "Database".into(), e.to_string())],
                    ..Default::default()
                };
            }
        };

        // Safeguard #12b: Group eligible mangas by `source_id` so we never hammer a single site
        // with parallel requests, while polling up to `max_parallel_sources` distinct sites concurrently!
        let mut by_source: BTreeMap<i64, Vec<LibraryMangaItem>> = BTreeMap::new();
        for item in eligible {
            by_source.entry(item.source_id).or_default().push(item);
        }

        let max_permits = config.max_parallel_sources.max(1);
        let semaphore = Arc::new(Semaphore::new(max_permits));
        let mut join_handles = Vec::with_capacity(by_source.len());

        for (source_id, mangas_in_source) in by_source {
            let source_opt = ext_mgr.get_source(source_id);
            let sem = semaphore.clone();

            join_handles.push(tokio::spawn(async move {
                let _permit = sem.acquire().await.ok();
                let mut source_results: Vec<(i64, String, Result<Vec<SChapter>, String>)> =
                    Vec::with_capacity(mangas_in_source.len());

                for item in mangas_in_source {
                    match &source_opt {
                        Some(src) => {
                            let res = src
                                .get_chapter_list(&item.manga)
                                .await
                                .map_err(|e| e.to_string());
                            source_results.push((item.id, item.manga.title, res));
                        }
                        None => {
                            source_results.push((
                                item.id,
                                item.manga.title,
                                Err(format!("Source {source_id} not installed")),
                            ));
                        }
                    }
                }
                source_results
            }));
        }

        let mut summary = LibraryUpdateSummary {
            was_already_running: false,
            mangas_checked: 0,
            new_drops: Vec::new(),
            skipped,
            failed: Vec::new(),
        };

        for handle in join_handles {
            if let Ok(source_results) = handle.await {
                for (manga_id, title, res) in source_results {
                    // Re-verify manga is still favorited in DB before committing chapters
                    let still_fav = db
                        .get_manga_by_id(manga_id)
                        .ok()
                        .flatten()
                        .map(|m| m.favorite_at > 0)
                        .unwrap_or(false);
                    if !still_fav {
                        continue;
                    }

                    match res {
                        Ok(remote_chapters) => {
                            summary.mangas_checked += 1;
                            match db.sync_chapters_detecting_new(manga_id, &remote_chapters, now_ms)
                            {
                                Ok(new_chs) => {
                                    if !new_chs.is_empty() {
                                        summary.new_drops.push((manga_id, title, new_chs));
                                    }
                                }
                                Err(e) => {
                                    summary.failed.push((manga_id, title, e.to_string()));
                                }
                            }
                        }
                        Err(err_msg) => {
                            summary.failed.push((manga_id, title, err_msg));
                        }
                    }
                }
            }
        }

        summary
    }
}
