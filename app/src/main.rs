//! # IKAZUCHI (雷) — Multi-Platform Manga & Manhwa Reader
//!
//! Main application entrypoint connecting:
//! - Phase 1: `ikazuchi-core` (Mihon domain models, deterministic Source ID & shard hashes)
//! - Phase 2: `ikazuchi-dex` (Pure-Rust `.apk` + `classes.dex` inspector & native scrapers)
//! - Phase 3: `ikazuchi-db` & `ikazuchi-archive` (Mihon-compatible SQLite engine & `.cbz` streamer)
//! - Phase 4: `protocol`, `state`, `theme`, `ui` (5 Realms UI & Raijin Reader Engine + `ikazuchi://` URI protocol)
//! - Phase 5: `ikazuchi-sync` (Serverless Private GitHub Repo Sync & deterministic CRDT reconciliation)

pub mod protocol;
pub mod state;
pub mod theme;
pub mod ui;
pub mod updater;

use crate::state::{AppSnapshot, ReaderSession, Realm, seed_initial_sanctuary};
use crate::ui::render_app_shell;
use ikazuchi_core::{AppConfig, Page};
use ikazuchi_db::Database;
use ikazuchi_dex::{ExtensionManager, build_synthetic_apk, build_synthetic_dex};
use ikazuchi_sync::{export_database_shards, reconcile_sync_trees};
use std::collections::BTreeMap;

#[tokio::main]
async fn main() {
    println!("⚡ IKAZUCHI (雷) — Initializing All 5 Phases...");

    // 0. Load (or initialize) persistent AppConfig (`config.json`)
    let config_path = std::env::temp_dir().join("ikazuchi_config.json");
    let config = AppConfig::load_from_file(&config_path).unwrap_or_default();
    config
        .save_to_file(&config_path)
        .expect("Failed to persist config.json");

    // 1. Initialize SQLite Database (`ikazuchi-db`) & Seed Sanctuary
    let db = Database::open_in_memory().expect("Failed to initialize Ikazuchi SQLite schema");
    let mut ext_mgr = ExtensionManager::new();
    seed_initial_sanctuary(&db, &mut ext_mgr);

    // 2. Test loading a Mihon .apk package through the pure-Rust DEX inspector (`ikazuchi-dex`)
    let manifest_xml = r#"
    <manifest package="eu.kanade.tachiyomi.extension.en.flamecomics">
        <uses-feature android:name="tachiyomi.extension" />
        <application>
            <meta-data android:name="tachiyomi.extension.class" android:value=".FlameComics" />
        </application>
    </manifest>
    "#;
    let dex_bytes = build_synthetic_dex(
        &[
            "FlameComics.kt",
            "Leu/kanade/tachiyomi/extension/en/flamecomics/FlameComics;",
            "Leu/kanade/tachiyomi/multisrc/mangathemesia/MangaThemesia;",
            "https://flamecomics.xyz",
        ],
        &[1, 2],
        &[(0, 1, 0)],
    );
    let apk_buf = build_synthetic_apk(manifest_xml, &dex_bytes);
    let (pkg_name, theme) = {
        let loaded = ext_mgr
            .load_apk_from_bytes(&apk_buf, None)
            .expect("Failed to load APK extension");
        (loaded.pkg_name.clone(), loaded.theme.clone())
    };

    println!(
        "  [Phase 1 & 2] Loaded Mihon APK '{}' -> Theme: {:?}, Visible Sources (NSFW={}): {}",
        pkg_name,
        theme,
        config.extensions.show_nsfw,
        ext_mgr.visible_sources(&config.extensions).len()
    );

    // 3. Verify Database Library, Categories, Updates & History (`ikazuchi-db`)
    let snapshot = AppSnapshot::load_from_with_config(&db, &ext_mgr, config.clone());
    println!(
        "  [Phase 3] SQLite Sanctuary -> {} Mangas, {} Categories ({}), {} Recent Drops, {} History Entries",
        snapshot.library.len(),
        snapshot.categories.len(),
        snapshot.categories.join(", "),
        snapshot.updates.len(),
        snapshot.history.len()
    );

    // 4. Verify UI Realms & Raijin Reader Engine (`ikazuchi/app`)
    let sanctuary_view = render_app_shell(&Realm::Sanctuary, &snapshot, &[], None);
    let session = ReaderSession::from_settings(
        snapshot.library[0].id,
        snapshot.library[0].manga.title.clone(),
        1,
        "Chapter 202 - Finale".into(),
        vec![Page {
            index: 0,
            url: String::new(),
            image_url: Some("ikazuchi://localhost/cbz/0?path=/tmp/demo.cbz".into()),
        }],
        0,
        &config.reader,
    );
    let reader_view = render_app_shell(
        &Realm::RaijinReader {
            manga_id: session.manga_id,
            chapter_id: 1,
        },
        &snapshot,
        &[],
        Some(&session),
    );
    println!(
        "  [Phase 4] Rendered Sanctuary UI ({} bytes) & Raijin Webtoon Reader ({} bytes)",
        sanctuary_view.len(),
        reader_view.len()
    );

    // 5. Verify Serverless GitHub Sync Shards & CRDT Reconciliation (`ikazuchi-sync`)
    let (_local_index, local_shards) = export_database_shards(&db).expect("Failed to export shards");
    let plan = reconcile_sync_trees(&local_shards, &BTreeMap::new());
    println!(
        "  [Phase 5] GitHub Sync Engine -> {} Shards Ready for Atomic Git Tree Push (library/index.json + library/mangas/*.json)",
        plan.to_push_remotely.len()
    );

    // 6. Verify Mihon `.tachibk` (.proto.gz) Backup Export & Import Round-Trip (`ikazuchi-db::backup`)
    let tachibk_bytes = db
        .export_mihon_backup_bytes(true)
        .expect("Failed to export Mihon .tachibk backup");
    let restored_db = Database::open_in_memory().unwrap();
    let import_report = restored_db
        .import_mihon_backup_bytes(&tachibk_bytes, 1_729_530_000_000)
        .expect("Failed to import Mihon .tachibk backup");
    println!(
        "  [Mihon Parity] Exported & Restored .tachibk ({} bytes Gzipped Protobuf) -> {} Mangas, {} Chapters, {} Categories",
        tachibk_bytes.len(),
        import_report.mangas_restored,
        import_report.chapters_restored,
        import_report.categories_restored
    );
    println!("⚡ IKAZUCHI (雷) All Phases Operational.");
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use ikazuchi_core::{MangaStatus, SChapter, SManga, compute_manga_sync_hash, generate_source_id};
    use ikazuchi_sync::{
        ChapterSyncState, MangaSyncState, apply_synced_shard_to_db, merge_manga_sync_state,
    };

    #[test]
    fn test_end_to_end_extension_to_db_and_github_sync_flow() {
        let db_desktop = Database::open_in_memory().unwrap();
        let db_android = Database::open_in_memory().unwrap();
        let source_id = generate_source_id("Asura Scans", "en", 1);

        let manga = SManga {
            url: "/manga/solo-leveling".into(),
            title: "Solo Leveling".into(),
            status: MangaStatus::Completed,
            initialized: true,
            ..Default::default()
        };

        let manga_id = db_desktop.upsert_manga(source_id, &manga, Some(1000), 1000).unwrap();
        db_desktop
            .set_manga_categories_by_name(manga_id, &["Manhwa".into(), "Completed".into()], 1000)
            .unwrap();

        let remote_chapters = vec![
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
        db_desktop.sync_chapters(manga_id, &remote_chapters, 2000).unwrap();

        let chs = db_desktop.get_chapters_by_manga_id(manga_id).unwrap();
        db_desktop
            .update_chapter_progress(chs[1].id, true, false, 40, 3000)
            .unwrap();

        let manga_hash = compute_manga_sync_hash(source_id, &manga.url);
        assert_eq!(manga_hash.len(), 16);

        // Export Desktop shards and simulate Android reading Chapter 2 up to page 19
        let (desktop_idx, desktop_shards) = export_database_shards(&db_desktop).unwrap();
        let desktop_state = desktop_shards.get(&manga_hash).unwrap().clone();

        let mut phone_map = desktop_state.chapters.clone();
        phone_map.insert(
            "/ch/2".into(),
            ChapterSyncState {
                name: "Chapter 2".into(),
                chapter_number: 2.0,
                scanlator: Some("Asura".into()),
                read: false,
                bookmark: true,
                last_page_read: 19,
                updated_at: 4500,
            },
        );
        let phone_state = MangaSyncState {
            chapters: phone_map,
            updated_at: 4500,
            ..desktop_state.clone()
        };

        let merged = merge_manga_sync_state(&desktop_state, &phone_state);
        assert!(merged.chapters["/ch/1"].read);
        assert_eq!(merged.chapters["/ch/2"].last_page_read, 19);
        assert!(merged.chapters["/ch/2"].bookmark);

        // Apply merged shard onto Android's blank database and verify full restoration
        apply_synced_shard_to_db(&db_android, &merged).unwrap();
        let android_lib = db_android.get_library_manga().unwrap();
        assert_eq!(android_lib.len(), 1);
        assert_eq!(android_lib[0].manga.title, "Solo Leveling");
        assert_eq!(android_lib[0].total_chapters, 2);
        assert_eq!(android_lib[0].unread_count, 1);
        assert_eq!(desktop_idx.entries.len(), 1);
    }

    #[tokio::test]
    async fn test_manual_and_scheduled_library_updater_with_safeguards() {
        use crate::updater::LibraryUpdater;
        use ikazuchi_archive::write_cbz_archive;
        use ikazuchi_core::UpdateStrategy;
        use ikazuchi_db::{LibraryUpdateConfig, UpdateSkipReason};
        use ikazuchi_dex::{LOCAL_SOURCE_ID, LocalSource};
        use std::sync::Arc;

        let local_root = std::env::temp_dir().join("ikazuchi_library_updater_test");
        let _ = std::fs::remove_dir_all(&local_root);
        let manga_dir = local_root.join("Kagurabachi");
        std::fs::create_dir_all(&manga_dir).unwrap();

        let png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 1, 2, 3];
        write_cbz_archive(
            manga_dir.join("Chapter 1.cbz"),
            &[("001.png".into(), png.clone())],
        )
        .unwrap();

        let db = Database::open_in_memory().unwrap();
        let mut ext_mgr = ExtensionManager::new();
        ext_mgr.register_source(Arc::new(LocalSource::new(&local_root)));

        // 1. Add "Kagurabachi" (Ongoing, on LocalSource) and "Monster" (Completed, should be skipped!)
        let kagurabachi = SManga {
            url: "Kagurabachi".into(),
            title: "Kagurabachi".into(),
            status: MangaStatus::Ongoing,
            update_strategy: UpdateStrategy::AlwaysUpdate,
            initialized: true,
            ..Default::default()
        };
        let k_id = db
            .upsert_manga(LOCAL_SOURCE_ID, &kagurabachi, Some(1000), 1000)
            .unwrap();

        let monster = SManga {
            url: "Monster".into(),
            title: "Monster".into(),
            status: MangaStatus::Completed,
            update_strategy: UpdateStrategy::AlwaysUpdate,
            initialized: true,
            ..Default::default()
        };
        let m_id = db
            .upsert_manga(LOCAL_SOURCE_ID, &monster, Some(1000), 1000)
            .unwrap();

        let updater = LibraryUpdater::new();
        let cfg = LibraryUpdateConfig::default();

        // First scheduled/manual update: "Monster" is skipped (Completed), "Kagurabachi" discovers Chapter 1!
        let summary1 = updater.run_update(&db, &ext_mgr, &cfg, None, 2000).await;
        assert!(!summary1.was_already_running);
        assert_eq!(summary1.mangas_checked, 1);
        assert_eq!(summary1.total_new_chapters(), 1);
        assert_eq!(
            summary1.skipped,
            vec![(m_id, "Monster".to_string(), UpdateSkipReason::Completed)]
        );

        // Now drop a brand new "Chapter 2.cbz" into the folder and trigger manual single-manga update!
        write_cbz_archive(
            manga_dir.join("Chapter 2.cbz"),
            &[("001.png".into(), png)],
        )
        .unwrap();

        let new_drops = updater
            .update_single_manga(&db, &ext_mgr, k_id, true, 3000)
            .await
            .unwrap();
        assert_eq!(new_drops.len(), 1);
        assert_eq!(new_drops[0].chapter.chapter_number, 2.0);

        let _ = std::fs::remove_dir_all(&local_root);
    }

    #[test]
    fn test_app_config_controls_nsfw_filtering_reader_defaults_and_updater_rules() {
        use crate::state::ReaderMode;
        use ikazuchi_core::ReadingModeSetting;
        use ikazuchi_db::LibraryUpdateConfig;
        use ikazuchi_dex::MadaraSource;
        use std::sync::Arc;

        let cfg_path = std::env::temp_dir().join("ikazuchi_integration_config.json");
        let _ = std::fs::remove_file(&cfg_path);

        let db = Database::open_in_memory().unwrap();
        let mut ext_mgr = ExtensionManager::new();
        seed_initial_sanctuary(&db, &mut ext_mgr);

        // Register an 18+ NSFW source alongside the safe seeded sources
        let nsfw_src = Arc::new(MadaraSource::new("HentaiVault", "https://nsfw.example", "en", 1));
        ext_mgr.register_source_with_nsfw(nsfw_src, true);

        // 1. Default config (`show_nsfw = false`): HentaiVault is hidden from AppSnapshot!
        let mut cfg = AppConfig::load_from_file(&cfg_path).unwrap();
        let snap_safe = AppSnapshot::load_from_with_config(&db, &ext_mgr, cfg.clone());
        assert_eq!(snap_safe.installed_sources.len(), 3);
        assert!(
            !snap_safe
                .installed_sources
                .iter()
                .any(|(_, name, _, _)| name == "HentaiVault")
        );

        // 2. Update settings (`show_nsfw = true`, `PagedRtl`, `zoom = 80%`, `auto_update = 6h`), save to disk & reload
        cfg.extensions.show_nsfw = true;
        cfg.reader.default_reading_mode = ReadingModeSetting::PagedRtl;
        cfg.reader.default_zoom_width_pct = 80;
        cfg.library.auto_update_interval_hours = 6;
        cfg.library.excluded_categories = vec!["On Hold".into()];
        cfg.save_to_file(&cfg_path).unwrap();

        let reloaded_cfg = AppConfig::load_from_file(&cfg_path).unwrap();
        let snap_nsfw = AppSnapshot::load_from_with_config(&db, &ext_mgr, reloaded_cfg.clone());
        assert_eq!(snap_nsfw.installed_sources.len(), 4);
        assert!(
            snap_nsfw
                .installed_sources
                .iter()
                .any(|(_, name, _, _)| name == "HentaiVault")
        );

        // 3. Verify ReaderSession & LibraryUpdateConfig inherit settings from reloaded `config.json`
        let reader_session = ReaderSession::from_settings(
            1,
            "Vagabond".into(),
            1,
            "Chapter 327".into(),
            vec![Page {
                index: 0,
                url: String::new(),
                image_url: Some("https://example.org/1.png".into()),
            }],
            0,
            &reloaded_cfg.reader,
        );
        assert_eq!(reader_session.mode, ReaderMode::PagedRtl);
        assert_eq!(reader_session.zoom_width_pct, 80);

        let update_cfg = LibraryUpdateConfig::from_app_config(&reloaded_cfg);
        assert_eq!(update_cfg.min_update_interval_ms, 6 * 3_600_000);
        assert_eq!(update_cfg.excluded_categories, vec!["On Hold"]);

        let vault_html = render_app_shell(&Realm::Vault, &snap_nsfw, &[], None);
        assert!(vault_html.contains("VISIBLE (18+ ENABLED)"));
        assert!(vault_html.contains("Every 6h"));

        let _ = std::fs::remove_file(&cfg_path);
    }

    #[tokio::test]
    async fn test_concurrent_library_update_single_flight_lock() {
        use crate::updater::LibraryUpdater;
        use ikazuchi_db::LibraryUpdateConfig;
        use std::sync::atomic::Ordering;

        let db = Database::open_in_memory().unwrap();
        let ext_mgr = ExtensionManager::new();
        let updater = LibraryUpdater::new();
        let cfg = LibraryUpdateConfig::default();

        // Simulate an in-flight background scheduled update by locking `updater.is_running`
        let running_flag = updater.is_running_flag();
        running_flag.store(true, Ordering::SeqCst);
        assert!(updater.is_updating());

        // A manual or scheduled update attempt while running MUST return was_already_running: true
        let summary_locked = updater.run_update(&db, &ext_mgr, &cfg, None, 1000).await;
        assert!(summary_locked.was_already_running);
        assert_eq!(summary_locked.mangas_checked, 0);

        // Once the in-flight update finishes, updater becomes available again
        running_flag.store(false, Ordering::SeqCst);
        assert!(!updater.is_updating());

        let summary_free = updater.run_update(&db, &ext_mgr, &cfg, None, 1000).await;
        assert!(!summary_free.was_already_running);
    }
}
