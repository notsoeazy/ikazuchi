//! # `ikazuchi-sync`
//!
//! Serverless Private GitHub Repository Sync & Deterministic CRDT Conflict Resolution:
//! - Exports and imports sharded per-manga states (`library/mangas/<manga_hash>.json`) to/from `ikazuchi-db`.
//! - Reconciles local vs. remote shard trees using deterministic CRDT merge rules (`reconcile_sync_trees`).
//! - Commits all dirty shards and `library/index.json` in a single atomic Git commit via GitHub's Git Data REST API (`GithubSyncClient`).

use ikazuchi_core::{IkazuchiError, MangaStatus, Result, SChapter, SManga, compute_manga_sync_hash};
use ikazuchi_db::Database;
use reqwest::Client;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Synced state of a single chapter inside `<manga_hash>.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChapterSyncState {
    pub name: String,
    pub chapter_number: f32,
    pub scanlator: Option<String>,
    pub read: bool,
    pub bookmark: bool,
    pub last_page_read: u32,
    pub updated_at: i64,
}

/// Sharded per-manga sync state stored at `library/mangas/<manga_hash>.json` in the user's private GitHub repo.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MangaSyncState {
    pub manga_hash: String,
    pub source_id: i64,
    pub remote_url: String,
    pub title: String,
    pub author: Option<String>,
    pub artist: Option<String>,
    pub description: Option<String>,
    pub genre: Vec<String>,
    pub status: i32,
    pub cover_url: Option<String>,
    pub favorite_at: Option<i64>,
    pub categories: Vec<String>,
    pub reader_flags: i32,
    pub updated_at: i64,
    pub chapters: BTreeMap<String, ChapterSyncState>,
}

/// Entry in `library/index.json` summarizing a manga shard's latest revision timestamp.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibraryIndexEntry {
    pub manga_hash: String,
    pub title: String,
    pub source_id: i64,
    pub updated_at: i64,
}

/// Top-level `library/index.json` file stored in the private GitHub sync repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct LibraryIndex {
    pub version: u32,
    pub updated_at: i64,
    pub entries: BTreeMap<String, LibraryIndexEntry>,
}

/// Synced extension repositories and installed extension package names (`settings/extensions.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ExtensionsSyncState {
    pub repo_urls: Vec<String>,
    pub installed_packages: Vec<String>,
    pub updated_at: i64,
}

/// Output of reconciling local vs. remote `MangaSyncState` shard maps.
#[derive(Debug, Clone)]
pub struct SyncReconciliation {
    /// Merged shards that need to be written into the local SQLite database.
    pub to_apply_locally: BTreeMap<String, MangaSyncState>,
    /// Merged shards that need to be pushed to the remote GitHub repository.
    pub to_push_remotely: BTreeMap<String, MangaSyncState>,
    /// Updated unified `LibraryIndex`.
    pub unified_index: LibraryIndex,
}

/// Deterministic CRDT conflict resolver for merging local and remote GitHub states of the same manga.
pub fn merge_manga_sync_state(local: &MangaSyncState, remote: &MangaSyncState) -> MangaSyncState {
    let primary = if remote.updated_at > local.updated_at {
        remote
    } else {
        local
    };

    let mut merged_chapters = local.chapters.clone();

    for (ch_url, rem_ch) in &remote.chapters {
        match merged_chapters.get_mut(ch_url) {
            Some(loc_ch) => {
                let read = loc_ch.read || rem_ch.read;
                let last_page_read = loc_ch.last_page_read.max(rem_ch.last_page_read);
                let bookmark = if rem_ch.updated_at > loc_ch.updated_at {
                    rem_ch.bookmark
                } else {
                    loc_ch.bookmark
                };
                let name = if !rem_ch.name.is_empty() && rem_ch.updated_at >= loc_ch.updated_at {
                    rem_ch.name.clone()
                } else {
                    loc_ch.name.clone()
                };

                *loc_ch = ChapterSyncState {
                    name,
                    chapter_number: rem_ch.chapter_number,
                    scanlator: rem_ch.scanlator.clone().or_else(|| loc_ch.scanlator.clone()),
                    read,
                    bookmark,
                    last_page_read,
                    updated_at: loc_ch.updated_at.max(rem_ch.updated_at),
                };
            }
            None => {
                merged_chapters.insert(ch_url.clone(), rem_ch.clone());
            }
        }
    }

    let max_ch_updated = merged_chapters.values().map(|c| c.updated_at).max().unwrap_or(0);
    let unified_updated_at = local.updated_at.max(remote.updated_at).max(max_ch_updated);

    MangaSyncState {
        manga_hash: primary.manga_hash.clone(),
        source_id: primary.source_id,
        remote_url: primary.remote_url.clone(),
        title: primary.title.clone(),
        author: primary.author.clone().or_else(|| local.author.clone()),
        artist: primary.artist.clone().or_else(|| local.artist.clone()),
        description: primary.description.clone().or_else(|| local.description.clone()),
        genre: if !primary.genre.is_empty() {
            primary.genre.clone()
        } else {
            local.genre.clone()
        },
        status: primary.status,
        cover_url: primary.cover_url.clone().or_else(|| local.cover_url.clone()),
        favorite_at: primary.favorite_at,
        categories: primary.categories.clone(),
        reader_flags: primary.reader_flags,
        updated_at: unified_updated_at,
        chapters: merged_chapters,
    }
}

/// Reconciles all local manga shards against remote GitHub shards, computing the minimal delta
/// of shards to apply locally and shards to push to GitHub.
pub fn reconcile_sync_trees(
    local_shards: &BTreeMap<String, MangaSyncState>,
    remote_shards: &BTreeMap<String, MangaSyncState>,
) -> SyncReconciliation {
    let mut to_apply_locally = BTreeMap::new();
    let mut to_push_remotely = BTreeMap::new();
    let mut index_entries = BTreeMap::new();
    let mut max_ts = 0i64;

    let mut all_hashes: Vec<&String> = local_shards.keys().chain(remote_shards.keys()).collect();
    all_hashes.sort();
    all_hashes.dedup();

    for hash in all_hashes {
        let unified = match (local_shards.get(hash), remote_shards.get(hash)) {
            (Some(loc), Some(rem)) => {
                let merged = merge_manga_sync_state(loc, rem);
                if &merged != loc {
                    to_apply_locally.insert(hash.clone(), merged.clone());
                }
                if &merged != rem {
                    to_push_remotely.insert(hash.clone(), merged.clone());
                }
                merged
            }
            (Some(loc), None) => {
                to_push_remotely.insert(hash.clone(), loc.clone());
                loc.clone()
            }
            (None, Some(rem)) => {
                to_apply_locally.insert(hash.clone(), rem.clone());
                rem.clone()
            }
            (None, None) => unreachable!(),
        };

        max_ts = max_ts.max(unified.updated_at);
        index_entries.insert(
            hash.clone(),
            LibraryIndexEntry {
                manga_hash: hash.clone(),
                title: unified.title.clone(),
                source_id: unified.source_id,
                updated_at: unified.updated_at,
            },
        );
    }

    SyncReconciliation {
        to_apply_locally,
        to_push_remotely,
        unified_index: LibraryIndex {
            version: 1,
            updated_at: max_ts,
            entries: index_entries,
        },
    }
}

/// Exports all favorited library mangas and their chapters from `Database` into sharded `MangaSyncState` records.
pub fn export_database_shards(db: &Database) -> Result<(LibraryIndex, BTreeMap<String, MangaSyncState>)> {
    let library_items = db.get_library_manga()?;
    let mut shards = BTreeMap::new();
    let mut entries = BTreeMap::new();
    let mut max_ts = 0i64;

    for item in library_items {
        let manga_hash = compute_manga_sync_hash(item.source_id, &item.manga.url);
        let db_chapters = db.get_chapters_by_manga_id(item.id)?;
        let mut ch_map = BTreeMap::new();
        let mut latest_ch_ts = item.sync_updated_at;

        for ch in db_chapters {
            latest_ch_ts = latest_ch_ts.max(ch.sync_updated_at);
            ch_map.insert(
                ch.chapter.url.clone(),
                ChapterSyncState {
                    name: ch.chapter.name,
                    chapter_number: ch.chapter.chapter_number,
                    scanlator: ch.chapter.scanlator,
                    read: ch.read,
                    bookmark: ch.bookmark,
                    last_page_read: ch.last_page_read,
                    updated_at: ch.sync_updated_at,
                },
            );
        }

        let shard = MangaSyncState {
            manga_hash: manga_hash.clone(),
            source_id: item.source_id,
            remote_url: item.manga.url,
            title: item.manga.title.clone(),
            author: item.manga.author,
            artist: item.manga.artist,
            description: item.manga.description,
            genre: item.manga.genre,
            status: item.manga.status as i32,
            cover_url: item.manga.thumbnail_url,
            favorite_at: Some(item.favorite_at),
            categories: item.categories,
            reader_flags: item.reader_flags,
            updated_at: latest_ch_ts,
            chapters: ch_map,
        };

        max_ts = max_ts.max(latest_ch_ts);
        entries.insert(
            manga_hash.clone(),
            LibraryIndexEntry {
                manga_hash: manga_hash.clone(),
                title: item.manga.title,
                source_id: item.source_id,
                updated_at: latest_ch_ts,
            },
        );
        shards.insert(manga_hash, shard);
    }

    Ok((
        LibraryIndex {
            version: 1,
            updated_at: max_ts,
            entries,
        },
        shards,
    ))
}

/// Applies a merged `MangaSyncState` shard into the local SQLite `Database`.
pub fn apply_synced_shard_to_db(db: &Database, shard: &MangaSyncState) -> Result<i64> {
    let smanga = SManga {
        url: shard.remote_url.clone(),
        title: shard.title.clone(),
        author: shard.author.clone(),
        artist: shard.artist.clone(),
        description: shard.description.clone(),
        genre: shard.genre.clone(),
        status: MangaStatus::from(shard.status),
        thumbnail_url: shard.cover_url.clone(),
        update_strategy: Default::default(),
        initialized: true,
    };

    let manga_id = db.upsert_manga(shard.source_id, &smanga, shard.favorite_at, shard.updated_at)?;
    db.set_manga_reader_flags(manga_id, shard.reader_flags, shard.updated_at)?;
    db.set_manga_categories_by_name(manga_id, &shard.categories, shard.updated_at)?;

    let mut remote_chapters: Vec<SChapter> = shard
        .chapters
        .iter()
        .map(|(url, ch)| SChapter {
            url: url.clone(),
            name: if ch.name.is_empty() {
                format!("Ch. {}", ch.chapter_number)
            } else {
                ch.name.clone()
            },
            chapter_number: ch.chapter_number,
            date_upload: ch.updated_at,
            scanlator: ch.scanlator.clone(),
        })
        .collect();
    remote_chapters.sort_by(|a, b| b.chapter_number.total_cmp(&a.chapter_number));

    db.sync_chapters(manga_id, &remote_chapters, shard.updated_at)?;

    let stored_chapters = db.get_chapters_by_manga_id(manga_id)?;
    for stored in stored_chapters {
        if let Some(ch_sync) = shard.chapters.get(&stored.chapter.url) {
            if stored.read != ch_sync.read
                || stored.bookmark != ch_sync.bookmark
                || stored.last_page_read != ch_sync.last_page_read
            {
                db.update_chapter_progress(
                    stored.id,
                    ch_sync.read,
                    ch_sync.bookmark,
                    ch_sync.last_page_read,
                    ch_sync.updated_at,
                )?;
            }
        }
    }

    Ok(manga_id)
}

/// Configuration for connecting to a user's private GitHub sync repository.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GithubSyncConfig {
    pub owner: String,
    pub repo: String,
    pub branch: String,
    pub personal_access_token: String,
    #[serde(default = "default_github_api_url")]
    pub api_base_url: String,
}

fn default_github_api_url() -> String {
    "https://api.github.com".to_string()
}

/// Serverless GitHub REST API client that reads and commits sharded sync files via GitHub's Git Data API.
pub struct GithubSyncClient {
    pub config: GithubSyncConfig,
    client: Client,
}

#[derive(Deserialize)]
struct GitRefResponse {
    object: GitRefObject,
}

#[derive(Deserialize)]
struct GitRefObject {
    sha: String,
}

#[derive(Deserialize)]
struct GitCommitResponse {
    sha: String,
    tree: GitRefObject,
}

#[derive(Serialize)]
struct CreateTreeRequest<'a> {
    base_tree: &'a str,
    tree: Vec<GitTreeItem<'a>>,
}

#[derive(Serialize)]
struct GitTreeItem<'a> {
    path: &'a str,
    mode: &'static str,
    #[serde(rename = "type")]
    item_type: &'static str,
    content: &'a str,
}

#[derive(Deserialize)]
struct CreateTreeResponse {
    sha: String,
}

#[derive(Serialize)]
struct CreateCommitRequest<'a> {
    message: &'a str,
    tree: &'a str,
    parents: Vec<&'a str>,
}

#[derive(Serialize)]
struct UpdateRefRequest<'a> {
    sha: &'a str,
    force: bool,
}

impl GithubSyncClient {
    pub fn new(config: GithubSyncConfig) -> Self {
        let client = Client::builder()
            .user_agent("Ikazuchi-Sync/0.1.0")
            .build()
            .unwrap_or_default();
        Self { config, client }
    }

    /// Fetches a raw file from the private GitHub repository (`None` if `404 Not Found`).
    pub async fn fetch_file_raw(&self, path: &str) -> Result<Option<String>> {
        let url = format!(
            "{}/repos/{}/{}/contents/{}?ref={}",
            self.config.api_base_url.trim_end_matches('/'),
            self.config.owner,
            self.config.repo,
            path.trim_start_matches('/'),
            self.config.branch
        );

        let resp = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.config.personal_access_token))
            .header("Accept", "application/vnd.github.raw+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .send()
            .await
            .map_err(|e| IkazuchiError::Network(e.to_string()))?;

        if resp.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !resp.status().is_success() {
            return Err(IkazuchiError::Network(format!(
                "GitHub API returned {} for {}",
                resp.status(),
                path
            )));
        }

        let text = resp.text().await.map_err(|e| IkazuchiError::Network(e.to_string()))?;
        Ok(Some(text))
    }

    /// Fetches `library/index.json` from the user's private GitHub repository.
    pub async fn fetch_remote_index(&self) -> Result<LibraryIndex> {
        match self.fetch_file_raw("library/index.json").await? {
            Some(json) => serde_json::from_str(&json).map_err(|e| IkazuchiError::Parse(e.to_string())),
            None => Ok(LibraryIndex::default()),
        }
    }

    /// Fetches a single `library/mangas/<manga_hash>.json` shard from GitHub.
    pub async fn fetch_remote_shard(&self, manga_hash: &str) -> Result<Option<MangaSyncState>> {
        let path = format!("library/mangas/{manga_hash}.json");
        match self.fetch_file_raw(&path).await? {
            Some(json) => {
                let parsed = serde_json::from_str(&json).map_err(|e| IkazuchiError::Parse(e.to_string()))?;
                Ok(Some(parsed))
            }
            None => Ok(None),
        }
    }

    /// Commits multiple file updates (`path -> utf8_content`) in a single atomic Git commit on `config.branch`.
    pub async fn push_atomic_commit(&self, message: &str, files: &BTreeMap<String, String>) -> Result<String> {
        if files.is_empty() {
            return Ok(String::new());
        }

        let base = format!(
            "{}/repos/{}/{}",
            self.config.api_base_url.trim_end_matches('/'),
            self.config.owner,
            self.config.repo
        );
        let auth = format!("Bearer {}", self.config.personal_access_token);

        // 1. Get current branch HEAD commit SHA
        let ref_url = format!("{base}/git/ref/heads/{}", self.config.branch);
        let ref_resp: GitRefResponse = self
            .client
            .get(&ref_url)
            .header("Authorization", &auth)
            .header("X-GitHub-Api-Version", "2022-11-28")
            .send()
            .await
            .map_err(|e| IkazuchiError::Network(e.to_string()))?
            .error_for_status()
            .map_err(|e| IkazuchiError::Network(e.to_string()))?
            .json()
            .await
            .map_err(|e| IkazuchiError::Parse(e.to_string()))?;
        let head_commit_sha = ref_resp.object.sha;

        // 2. Get base tree SHA of HEAD commit
        let commit_url = format!("{base}/git/commits/{head_commit_sha}");
        let commit_resp: GitCommitResponse = self
            .client
            .get(&commit_url)
            .header("Authorization", &auth)
            .header("X-GitHub-Api-Version", "2022-11-28")
            .send()
            .await
            .map_err(|e| IkazuchiError::Network(e.to_string()))?
            .error_for_status()
            .map_err(|e| IkazuchiError::Network(e.to_string()))?
            .json()
            .await
            .map_err(|e| IkazuchiError::Parse(e.to_string()))?;

        // 3. Create new Git Tree with all updated files inline
        let tree_items: Vec<GitTreeItem<'_>> = files
            .iter()
            .map(|(path, content)| GitTreeItem {
                path: path.trim_start_matches('/'),
                mode: "100644",
                item_type: "blob",
                content: content.as_str(),
            })
            .collect();

        let create_tree_req = CreateTreeRequest {
            base_tree: &commit_resp.tree.sha,
            tree: tree_items,
        };

        let tree_resp: CreateTreeResponse = self
            .client
            .post(format!("{base}/git/trees"))
            .header("Authorization", &auth)
            .header("X-GitHub-Api-Version", "2022-11-28")
            .json(&create_tree_req)
            .send()
            .await
            .map_err(|e| IkazuchiError::Network(e.to_string()))?
            .error_for_status()
            .map_err(|e| IkazuchiError::Network(e.to_string()))?
            .json()
            .await
            .map_err(|e| IkazuchiError::Parse(e.to_string()))?;

        // 4. Create Commit pointing to the new tree
        let new_commit_req = CreateCommitRequest {
            message,
            tree: &tree_resp.sha,
            parents: vec![&head_commit_sha],
        };

        let new_commit_resp: GitCommitResponse = self
            .client
            .post(format!("{base}/git/commits"))
            .header("Authorization", &auth)
            .header("X-GitHub-Api-Version", "2022-11-28")
            .json(&new_commit_req)
            .send()
            .await
            .map_err(|e| IkazuchiError::Network(e.to_string()))?
            .error_for_status()
            .map_err(|e| IkazuchiError::Network(e.to_string()))?
            .json()
            .await
            .map_err(|e| IkazuchiError::Parse(e.to_string()))?;

        // 5. Advance branch reference to the new commit
        let update_ref_req = UpdateRefRequest {
            sha: &new_commit_resp.sha,
            force: false,
        };
        self.client
            .patch(&ref_url)
            .header("Authorization", &auth)
            .header("X-GitHub-Api-Version", "2022-11-28")
            .json(&update_ref_req)
            .send()
            .await
            .map_err(|e| IkazuchiError::Network(e.to_string()))?
            .error_for_status()
            .map_err(|e| IkazuchiError::Network(e.to_string()))?;

        Ok(new_commit_resp.sha)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_crdt_merge_combines_progress_from_phone_and_desktop() {
        let mut local_chapters = BTreeMap::new();
        local_chapters.insert(
            "/ch/1".to_string(),
            ChapterSyncState {
                name: "Chapter 1".into(),
                chapter_number: 1.0,
                scanlator: None,
                read: true,
                bookmark: false,
                last_page_read: 20,
                updated_at: 100,
            },
        );
        local_chapters.insert(
            "/ch/2".to_string(),
            ChapterSyncState {
                name: "Chapter 2".into(),
                chapter_number: 2.0,
                scanlator: None,
                read: false,
                bookmark: false,
                last_page_read: 5,
                updated_at: 110,
            },
        );

        let mut remote_chapters = BTreeMap::new();
        remote_chapters.insert(
            "/ch/2".to_string(),
            ChapterSyncState {
                name: "Chapter 2".into(),
                chapter_number: 2.0,
                scanlator: None,
                read: false,
                bookmark: true,
                last_page_read: 14,
                updated_at: 120,
            },
        );
        remote_chapters.insert(
            "/ch/3".to_string(),
            ChapterSyncState {
                name: "Chapter 3".into(),
                chapter_number: 3.0,
                scanlator: None,
                read: true,
                bookmark: false,
                last_page_read: 18,
                updated_at: 130,
            },
        );

        let local = MangaSyncState {
            manga_hash: "abc12345".into(),
            source_id: 1,
            remote_url: "/manga/test".into(),
            title: "Vagabond".into(),
            author: Some("Takehiko Inoue".into()),
            artist: None,
            description: None,
            genre: vec!["Seinen".into()],
            status: 1,
            cover_url: None,
            favorite_at: Some(100),
            categories: vec!["Seinen".into()],
            reader_flags: 0,
            updated_at: 110,
            chapters: local_chapters,
        };

        let remote = MangaSyncState {
            chapters: remote_chapters,
            updated_at: 130,
            ..local.clone()
        };

        let merged = merge_manga_sync_state(&local, &remote);
        assert_eq!(merged.chapters.len(), 3);
        assert!(merged.chapters["/ch/1"].read);
        assert_eq!(merged.chapters["/ch/2"].last_page_read, 14);
        assert!(merged.chapters["/ch/2"].bookmark);
        assert!(merged.chapters["/ch/3"].read);
    }

    #[test]
    fn test_bidirectional_db_export_reconcile_and_import_across_devices() {
        // Device A (Linux Desktop) has Berserk (Ch.1 read)
        let db_desktop = Database::open_in_memory().unwrap();
        let berserk = SManga {
            url: "/manga/berserk".into(),
            title: "Berserk".into(),
            author: Some("Kentaro Miura".into()),
            genre: vec!["Dark Fantasy".into()],
            status: MangaStatus::Ongoing,
            initialized: true,
            ..Default::default()
        };
        let b_id = db_desktop.upsert_manga(10, &berserk, Some(1000), 1000).unwrap();
        db_desktop
            .set_manga_categories_by_name(b_id, &["Masterpieces".into()], 1000)
            .unwrap();
        db_desktop
            .sync_chapters(
                b_id,
                &[
                    SChapter {
                        url: "/ch/2".into(),
                        name: "Chapter 2".into(),
                        chapter_number: 2.0,
                        date_upload: 1000,
                        scanlator: None,
                    },
                    SChapter {
                        url: "/ch/1".into(),
                        name: "Chapter 1".into(),
                        chapter_number: 1.0,
                        date_upload: 900,
                        scanlator: None,
                    },
                ],
                1000,
            )
            .unwrap();
        let desktop_chs = db_desktop.get_chapters_by_manga_id(b_id).unwrap();
        db_desktop
            .update_chapter_progress(desktop_chs[1].id, true, false, 45, 2000)
            .unwrap();

        // Device B (Android Phone) has Berserk (Ch.2 read to page 17) AND Kingdom (brand new manga!)
        let db_phone = Database::open_in_memory().unwrap();
        let b_phone_id = db_phone.upsert_manga(10, &berserk, Some(1000), 1000).unwrap();
        db_phone
            .sync_chapters(
                b_phone_id,
                &[
                    SChapter {
                        url: "/ch/2".into(),
                        name: "Chapter 2".into(),
                        chapter_number: 2.0,
                        date_upload: 1000,
                        scanlator: None,
                    },
                    SChapter {
                        url: "/ch/1".into(),
                        name: "Chapter 1".into(),
                        chapter_number: 1.0,
                        date_upload: 900,
                        scanlator: None,
                    },
                ],
                1000,
            )
            .unwrap();
        let phone_chs = db_phone.get_chapters_by_manga_id(b_phone_id).unwrap();
        db_phone
            .update_chapter_progress(phone_chs[0].id, false, true, 17, 3000)
            .unwrap();

        let kingdom = SManga {
            url: "/manga/kingdom".into(),
            title: "Kingdom".into(),
            author: Some("Yasuhisa Hara".into()),
            status: MangaStatus::Ongoing,
            initialized: true,
            ..Default::default()
        };
        db_phone.upsert_manga(10, &kingdom, Some(2500), 2500).unwrap();

        // Export shards from both devices and reconcile!
        let (_, desktop_shards) = export_database_shards(&db_desktop).unwrap();
        let (_, phone_shards) = export_database_shards(&db_phone).unwrap();

        let reconciliation = reconcile_sync_trees(&desktop_shards, &phone_shards);
        assert_eq!(reconciliation.unified_index.entries.len(), 2);

        // Apply reconciled shards back to Desktop DB
        for shard in reconciliation.to_apply_locally.values() {
            apply_synced_shard_to_db(&db_desktop, shard).unwrap();
        }

        // Desktop DB now has both Berserk (with Ch.1 read AND Ch.2 at page 17!) and Kingdom!
        let desktop_lib = db_desktop.get_library_manga().unwrap();
        assert_eq!(desktop_lib.len(), 2);

        let synced_berserk_chs = db_desktop.get_chapters_by_manga_id(b_id).unwrap();
        assert_eq!(synced_berserk_chs[0].last_page_read, 17);
        assert!(synced_berserk_chs[0].bookmark);
        assert!(synced_berserk_chs[1].read);
    }
}
