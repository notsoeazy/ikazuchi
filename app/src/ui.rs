//! # Ikazuchi UI Shell & Realm View Renderer
//!
//! Renders the 5 Realms of Ikazuchi (*Sanctuary*, *Storm*, *Horizon*, *Chronicle*, *Vault*)
//! and the *Raijin Reader Engine* using the Ikazuchi Design System (`theme::IKAZUCHI_CSS`).

use crate::state::{AppSnapshot, ReaderMode, ReaderSession, Realm};
use crate::theme::IKAZUCHI_CSS;
use ikazuchi_db::ChapterRecord;

/// Renders the complete Ikazuchi Shell HTML/RSX tree for a given [`Realm`] and [`AppSnapshot`].
pub fn render_app_shell(
    active_realm: &Realm,
    snapshot: &AppSnapshot,
    active_chapters: &[ChapterRecord],
    reader_session: Option<&ReaderSession>,
) -> String {
    if let (Realm::RaijinReader { .. }, Some(session)) = (active_realm, reader_session) {
        return render_raijin_reader(session);
    }

    let main_content = match active_realm {
        Realm::Sanctuary => render_sanctuary_realm(snapshot),
        Realm::Storm => render_storm_realm(snapshot),
        Realm::Horizon => render_horizon_realm(snapshot),
        Realm::Chronicle { manga_id } => render_chronicle_realm(*manga_id, snapshot, active_chapters),
        Realm::Vault => render_vault_realm(snapshot),
        Realm::RaijinReader { .. } => String::new(),
    };

    let nav_items = [
        ("sanctuary", "⚡", "SANCTUARY", matches!(active_realm, Realm::Sanctuary | Realm::Chronicle { .. })),
        ("storm", "🌩", "STORM", matches!(active_realm, Realm::Storm)),
        ("horizon", "🧭", "HORIZON", matches!(active_realm, Realm::Horizon)),
        ("vault", "⛩", "VAULT", matches!(active_realm, Realm::Vault)),
    ];

    let mut rail_html = String::new();
    for (id, icon, label, is_active) in nav_items {
        let active_cls = if is_active { "active" } else { "" };
        rail_html.push_str(&format!(
            r#"<button class="ik-nav-btn {active_cls}" data-realm="{id}" title="{label}">{icon} {label}</button>"#
        ));
    }

    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <title>IKAZUCHI (雷) — Mythological Manga &amp; Manhwa Reader</title>
  <style>{IKAZUCHI_CSS}</style>
</head>
<body>
  <div class="ik-shell">
    <nav class="ik-rail" aria-label="Ikazuchi Realms">
      <div class="ik-brand"><span class="ik-brand-kanji">雷</span><span class="ik-brand-title">IKAZUCHI</span></div>
      {rail_html}
    </nav>
    <main class="ik-main">
      {main_content}
    </main>
  </div>
</body>
</html>"#
    )
}

/// 1. **Sanctuary** — Library Grid, Dynamic Yokai Grouping (`ByCategory`, `ByStatus`, `BySource`, `ByGenre`, `ByAuthor`), Unread Lightning Badges.
pub fn render_sanctuary_realm(snapshot: &AppSnapshot) -> String {
    let grouped = snapshot.grouped_library();
    let mut pills = format!(
        r#"<span class="pill active">ALL REALMS ({:?})</span>"#,
        snapshot.config.library.group_mode
    );
    for (group_label, items) in &grouped {
        pills.push_str(&format!(
            r#"<span class="pill">{} ({})</span>"#,
            group_label,
            items.len()
        ));
    }

    let mut cards = String::new();
    for item in &snapshot.library {
        let m = &item.manga;
        let badge = if item.unread_count > 0 {
            format!(r#"<span class="ik-badge-unread">⚡ {}</span>"#, item.unread_count)
        } else {
            String::new()
        };
        let cats = if item.categories.is_empty() {
            "Uncategorized".to_string()
        } else {
            item.categories.join(" • ")
        };
        let read_count = item.total_chapters.saturating_sub(item.unread_count);
        cards.push_str(&format!(
            r#"<article class="ik-card" data-manga-id="{id}">
  <div class="ik-card-cover">{badge}</div>
  <div class="ik-card-body">
    <div class="ik-card-title">{title}</div>
    <div class="ik-card-sub">{cats} // {read}/{total} CH</div>
  </div>
</article>"#,
            id = item.id,
            title = m.title,
            read = read_count,
            total = item.total_chapters,
        ));
    }

    format!(
        r#"<header class="ik-header">
  <div>
    <div class="ik-card-sub">REALM I // SANCTUARY</div>
    <h1 class="ik-title">THE SANCTUARY (雷蔵)</h1>
  </div>
  <div class="pill-bar">{pills}</div>
</header>
<section class="ik-grid">
  {cards}
</section>"#
    )
}

/// 2. **Storm** — Recent Chapter Drops & Reading Timeline.
pub fn render_storm_realm(snapshot: &AppSnapshot) -> String {
    let mut updates_rows = String::new();
    for u in &snapshot.updates {
        let status = if u.chapter.read { "READ" } else { "⚡ NEW DROP" };
        updates_rows.push_str(&format!(
            r#"<div class="chapter-row" data-chapter-id="{}">
  <div>
    <strong>{}</strong> — {}
  </div>
  <span class="chapter-meta">{}</span>
</div>"#,
            u.chapter.id, u.manga_title, u.chapter.chapter.name, status
        ));
    }

    let mut history_rows = String::new();
    for h in &snapshot.history {
        history_rows.push_str(&format!(
            r#"<div class="chapter-row" data-chapter-id="{}">
  <div>
    <strong>{}</strong> — {}
  </div>
  <span class="chapter-meta">PAGE {} // RESUME ⚡</span>
</div>"#,
            h.chapter_id, h.manga_title, h.chapter_name, h.last_page_read + 1
        ));
    }

    format!(
        r#"<header class="realm-header">
  <div>
    <div class="realm-subtitle">REALM II // THE STORM</div>
    <h1 class="realm-title">RECENT DROPS &amp; CHRONICLES</h1>
  </div>
</header>
<div class="vault-card">
  <h3>⚡ RECENT CHAPTER DROPS</h3>
  <div class="chapter-list">{updates_rows}</div>
</div>
<div class="vault-card">
  <h3>⏳ CONTINUE READING TIMELINE</h3>
  <div class="chapter-list">{history_rows}</div>
</div>"#
    )
}

/// 3. **Horizon** — Multi-Source Catalog Search & 1-Click Mihon Extension Installer.
pub fn render_horizon_realm(snapshot: &AppSnapshot) -> String {
    let mut source_rows = String::new();
    for (id, name, lang, base_url) in &snapshot.installed_sources {
        source_rows.push_str(&format!(
            r#"<div class="chapter-row" data-source-id="{id}">
  <div>
    <strong>{name}</strong> <span class="pill">{lang}</span>
    <div class="chapter-meta">{base_url}</div>
  </div>
  <span class="chapter-meta">MIHON ID: {id}</span>
</div>"#
        ));
    }

    format!(
        r#"<header class="realm-header">
  <div>
    <div class="realm-subtitle">REALM III // THE HORIZON</div>
    <h1 class="realm-title">UNIVERSAL MIHON EXTENSION ENGINE</h1>
  </div>
</header>
<div class="vault-card">
  <h3>ACTIVE SOURCES ({count})</h3>
  <div class="chapter-list">{source_rows}</div>
</div>"#,
        count = snapshot.installed_sources.len()
    )
}

/// 4. **Chronicle** — Manga Detail Header, Metadata & Chapter List.
pub fn render_chronicle_realm(
    manga_id: i64,
    snapshot: &AppSnapshot,
    chapters: &[ChapterRecord],
) -> String {
    let title = snapshot
        .library
        .iter()
        .find(|i| i.id == manga_id)
        .map(|i| i.manga.title.as_str())
        .unwrap_or("Unknown Chronicle");

    let mut chapter_rows = String::new();
    for ch in chapters {
        let cls = if ch.read { "chapter-row read" } else { "chapter-row" };
        let badge = if ch.bookmark { "🔖 " } else { "" };
        chapter_rows.push_str(&format!(
            r#"<div class="{cls}" data-chapter-id="{id}">
  <div>{badge}<strong>{name}</strong></div>
  <span class="chapter-meta">CH #{num} // PAGE {page}</span>
</div>"#,
            id = ch.id,
            name = ch.chapter.name,
            num = ch.chapter.chapter_number,
            page = ch.last_page_read
        ));
    }

    format!(
        r#"<header class="realm-header">
  <div>
    <div class="realm-subtitle">REALM IV // THE CHRONICLE</div>
    <h1 class="realm-title">{title}</h1>
  </div>
</header>
<div class="chapter-list">{chapter_rows}</div>"#
    )
}

/// 5. **Vault** — Serverless Private GitHub Repository Sync, CRDT Shards & App Settings (`config.json`).
pub fn render_vault_realm(snapshot: &AppSnapshot) -> String {
    let mut shard_rows = String::new();
    for (hash, entry) in &snapshot.sync_index.entries {
        shard_rows.push_str(&format!(
            r#"<div class="chapter-row">
  <div>
    <strong>{}</strong>
    <div class="chapter-meta">library/mangas/{hash}.json</div>
  </div>
  <span class="chapter-meta">UPDATED_AT: {}</span>
</div>"#,
            entry.title, entry.updated_at
        ));
    }

    let cfg = &snapshot.config;
    let stats = &snapshot.stats;
    let nsfw_status = if cfg.extensions.show_nsfw {
        "VISIBLE (18+ ENABLED)"
    } else {
        "HIDDEN (SAFE MODE)"
    };
    let langs = cfg.extensions.enabled_languages.join(", ");

    format!(
        r#"<header class="realm-header">
  <div>
    <div class="realm-subtitle">REALM V // THE VAULT</div>
    <h1 class="realm-title">SETTINGS, STATS &amp; SERVERLESS GITHUB SYNC ENGINE</h1>
  </div>
</header>
<div class="vault-card">
  <h3>📊 LIBRARY &amp; READING STATISTICS (YOKAI PARITY)</h3>
  <p class="chapter-meta">LIBRARY ENTRIES: {total_mangas} // CHAPTERS READ: {read_chs} / {total_chs}</p>
  <p class="chapter-meta">TOTAL READ DURATION: {read_sec}s // TRACKED MANGAS: {tracked} (Mean Score: {mean_score:.1})</p>
</div>
<div class="vault-card">
  <h3>⚙ APPLICATION SETTINGS (config.json)</h3>
  <p class="chapter-meta">THEME: {theme:?} ({mode:?} // AMOLED: {amoled})</p>
  <p class="chapter-meta">LIBRARY GROUPING: {group:?} // SORT: {sort:?} (Ascending: {asc})</p>
  <p class="chapter-meta">EXTENSION FILTER: NSFW={nsfw} // LANGUAGES=[{langs}]</p>
  <p class="chapter-meta">SCHEDULED UPDATES: Every {interval}h (Ongoing Only: {ongoing})</p>
  <p class="chapter-meta">READER DEFAULT: {reader_mode:?} ({zoom}% Zoom, {pad}% Side Padding, Auto-Mark Duplicate Read: {dup_read})</p>
  <p class="chapter-meta">DOWNLOADS: CBZ={cbz} (ComicInfo.xml Embedded) // Parallel Sources={psrc}, Pages={ppage}</p>
</div>
<div class="vault-card">
  <h3>⛩ PRIVATE GITHUB REPOSITORY TARGET</h3>
  <p class="chapter-meta">REPO: {owner}/{repo} (branch: {branch})</p>
  <p class="chapter-meta">SHARDS TRACKED: {count} deterministic CRDT files</p>
</div>
<div class="vault-card">
  <h3>⚡ SHARDED MANGA CRDT MANIFEST (library/index.json)</h3>
  <div class="chapter-list">{shard_rows}</div>
</div>"#,
        total_mangas = stats.total_library_mangas,
        read_chs = stats.total_chapters_read,
        total_chs = stats.total_chapters,
        read_sec = stats.total_read_duration_sec,
        tracked = stats.tracked_mangas_count,
        mean_score = stats.mean_tracker_score,
        theme = cfg.ui.app_theme,
        mode = cfg.ui.theme_mode,
        amoled = cfg.ui.theme_dark_amoled,
        group = cfg.library.group_mode,
        sort = cfg.library.sort_mode,
        asc = cfg.library.sort_ascending,
        nsfw = nsfw_status,
        langs = langs,
        interval = cfg.library.auto_update_interval_hours,
        ongoing = cfg.library.update_only_ongoing,
        reader_mode = cfg.reader.default_reading_mode,
        zoom = cfg.reader.default_zoom_width_pct,
        pad = cfg.reader.webtoon_side_padding_pct,
        dup_read = cfg.reader.auto_mark_duplicate_chapters_read,
        cbz = cfg.downloads.save_chapters_as_cbz,
        psrc = cfg.downloads.parallel_source_limit,
        ppage = cfg.downloads.parallel_page_limit,
        owner = snapshot.github_config.owner,
        repo = snapshot.github_config.repo,
        branch = snapshot.github_config.branch,
        count = snapshot.sync_index.entries.len()
    )
}

/// ⚡ **Raijin Reader** — Fullscreen Canvas supporting Webtoon Continuous & Paged RTL/LTR.
pub fn render_raijin_reader(session: &ReaderSession) -> String {
    let body_content = match session.mode {
        ReaderMode::WebtoonContinuous => {
            let mut imgs = String::new();
            for page in &session.pages {
                let src = page.image_url.as_deref().unwrap_or("");
                imgs.push_str(&format!(
                    r#"<img src="{src}" data-page-index="{}" loading="lazy" alt="Page {}" />"#,
                    page.index,
                    page.index + 1
                ));
            }
            format!(
                r#"<div class="raijin-webtoon-strip" style="width: {}%;">{imgs}</div>"#,
                session.zoom_width_pct
            )
        }
        ReaderMode::PagedRtl | ReaderMode::PagedLtr => {
            let dir = if session.mode == ReaderMode::PagedRtl {
                "rtl"
            } else {
                "ltr"
            };
            let current_src = session
                .pages
                .get(session.current_page)
                .and_then(|p| p.image_url.as_deref())
                .unwrap_or("");
            format!(
                r#"<div class="raijin-paged-stage" dir="{dir}">
  <img src="{current_src}" data-page-index="{idx}" alt="Page {num}" style="max-height: 92vh; max-width: {zoom}%;" />
</div>"#,
                idx = session.current_page,
                num = session.current_page + 1,
                zoom = session.zoom_width_pct
            )
        }
    };

    let hud = if session.hud_visible {
        format!(
            r#"<div class="raijin-hud">
  <span>⚡ {title} — {chapter}</span>
  <span>{mode}</span>
  <span>PAGE {cur} / {total} ({zoom}%)</span>
</div>"#,
            title = session.manga_title,
            chapter = session.chapter_name,
            mode = session.mode.label(),
            cur = session.current_page + 1,
            total = session.pages.len(),
            zoom = session.zoom_width_pct,
        )
    } else {
        String::new()
    };

    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <title>RAIJIN READER // {title}</title>
  <style>{IKAZUCHI_CSS}</style>
</head>
<body>
  <div class="raijin-viewport">
    {body_content}
    {hud}
  </div>
</body>
</html>"#,
        title = session.manga_title
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::seed_initial_sanctuary;
    use ikazuchi_core::Page;
    use ikazuchi_db::Database;
    use ikazuchi_dex::ExtensionManager;

    #[test]
    fn test_render_all_five_realms_and_raijin_reader() {
        let db = Database::open_in_memory().unwrap();
        let mut ext_mgr = ExtensionManager::new();
        seed_initial_sanctuary(&db, &mut ext_mgr);

        let snapshot = AppSnapshot::load_from(&db, &ext_mgr);
        assert_eq!(snapshot.library.len(), 3);

        // 1. Sanctuary
        let sanctuary_html = render_app_shell(&Realm::Sanctuary, &snapshot, &[], None);
        assert!(sanctuary_html.contains("THE SANCTUARY"));
        assert!(sanctuary_html.contains("Vagabond"));
        assert!(sanctuary_html.contains("Legend of the Northern Blade"));

        // 2. Storm
        let storm_html = render_app_shell(&Realm::Storm, &snapshot, &[], None);
        assert!(storm_html.contains("RECENT DROPS"));
        assert!(storm_html.contains("CONTINUE READING TIMELINE"));

        // 3. Horizon
        let horizon_html = render_app_shell(&Realm::Horizon, &snapshot, &[], None);
        assert!(horizon_html.contains("UNIVERSAL MIHON EXTENSION ENGINE"));
        assert!(horizon_html.contains("Asura Scans"));

        // 4. Chronicle
        let first_id = snapshot.library[0].id;
        let chapters = AppSnapshot::get_chapters(&db, first_id);
        let chronicle_html = render_app_shell(
            &Realm::Chronicle { manga_id: first_id },
            &snapshot,
            &chapters,
            None,
        );
        assert!(chronicle_html.contains("REALM IV // THE CHRONICLE"));

        // 5. Vault
        let vault_html = render_app_shell(&Realm::Vault, &snapshot, &[], None);
        assert!(vault_html.contains("SERVERLESS GITHUB SYNC ENGINE"));
        assert!(vault_html.contains("library/mangas/"));

        // 6. Raijin Reader (Webtoon Continuous)
        let session = ReaderSession::new(
            first_id,
            "Legend of the Northern Blade".into(),
            1,
            "Chapter 202".into(),
            vec![
                Page {
                    index: 0,
                    url: String::new(),
                    image_url: Some("ikazuchi://localhost/cbz/0?path=/tmp/ch202.cbz".into()),
                },
                Page {
                    index: 1,
                    url: String::new(),
                    image_url: Some("ikazuchi://localhost/cbz/1?path=/tmp/ch202.cbz".into()),
                },
            ],
            0,
            ReaderMode::WebtoonContinuous,
        );
        let reader_html = render_app_shell(
            &Realm::RaijinReader {
                manga_id: first_id,
                chapter_id: 1,
            },
            &snapshot,
            &[],
            Some(&session),
        );
        assert!(reader_html.contains("raijin-webtoon-strip"));
        assert!(reader_html.contains("WEBTOON // VERTICAL STRIP"));
    }
}
