//! # MangaDex API v5 Native Rust Driver (`MangaDexSource`)
//!
//! Native Rust implementation of the MangaDex v5 REST API (`api.mangadex.org`),
//! matching Mihon's MangaDex extension behavior and deterministic Source ID.

use crate::drivers::fetch_text;
use async_trait::async_trait;
use ikazuchi_core::{
    FilterList, IkazuchiError, MangaSource, MangaStatus, MangasPage, Page, Result, SChapter, SManga, UpdateStrategy,
    generate_source_id,
};
use reqwest::Client;
use serde::Deserialize;
use std::collections::HashMap;

pub struct MangaDexSource {
    pub id: i64,
    pub lang: String,
    pub base_url: String,
    pub api_url: String,
    pub client: Client,
}

#[derive(Deserialize)]
struct MdMangaListResponse {
    data: Vec<MdMangaData>,
    limit: u32,
    offset: u32,
    total: u32,
}

#[derive(Deserialize)]
struct MdSingleMangaResponse {
    data: MdMangaData,
}

#[derive(Deserialize)]
struct MdMangaData {
    id: String,
    attributes: MdMangaAttributes,
    #[serde(default)]
    relationships: Vec<MdRelationship>,
}

#[derive(Deserialize)]
struct MdMangaAttributes {
    #[serde(default)]
    title: HashMap<String, String>,
    #[serde(default)]
    description: HashMap<String, String>,
    status: Option<String>,
    #[serde(default)]
    tags: Vec<MdTag>,
}

#[derive(Deserialize)]
struct MdTag {
    attributes: MdTagAttributes,
}

#[derive(Deserialize)]
struct MdTagAttributes {
    #[serde(default)]
    name: HashMap<String, String>,
}

#[derive(Deserialize)]
struct MdRelationship {
    #[serde(rename = "type")]
    rel_type: String,
    attributes: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct MdChapterListResponse {
    data: Vec<MdChapterData>,
}

#[derive(Deserialize)]
struct MdChapterData {
    id: String,
    attributes: MdChapterAttributes,
    #[serde(default)]
    relationships: Vec<MdRelationship>,
}

#[derive(Deserialize)]
struct MdChapterAttributes {
    title: Option<String>,
    chapter: Option<String>,
    volume: Option<String>,
}

#[derive(Deserialize)]
struct MdAtHomeResponse {
    #[serde(rename = "baseUrl")]
    base_url: String,
    chapter: MdAtHomeChapter,
}

#[derive(Deserialize)]
struct MdAtHomeChapter {
    hash: String,
    data: Vec<String>,
}

impl MangaDexSource {
    pub fn new(lang: impl Into<String>) -> Self {
        let lang = lang.into();
        let id = generate_source_id("MangaDex", &lang, 1);
        let client = Client::builder()
            .user_agent("Ikazuchi/0.1.0")
            .build()
            .unwrap_or_default();

        Self {
            id,
            lang,
            base_url: "https://mangadex.org".to_string(),
            api_url: "https://api.mangadex.org".to_string(),
            client,
        }
    }

    fn map_md_manga(&self, data: MdMangaData) -> SManga {
        let title = data
            .attributes
            .title
            .get(&self.lang)
            .or_else(|| data.attributes.title.get("en"))
            .or_else(|| data.attributes.title.values().next())
            .cloned()
            .unwrap_or_else(|| "Unknown Title".to_string());

        let description = data
            .attributes
            .description
            .get(&self.lang)
            .or_else(|| data.attributes.description.get("en"))
            .or_else(|| data.attributes.description.values().next())
            .cloned();

        let status = match data.attributes.status.as_deref() {
            Some("ongoing") => MangaStatus::Ongoing,
            Some("completed") => MangaStatus::Completed,
            Some("hiatus") => MangaStatus::OnHiatus,
            Some("cancelled") => MangaStatus::Cancelled,
            _ => MangaStatus::Unknown,
        };

        let genre: Vec<String> = data
            .attributes
            .tags
            .into_iter()
            .filter_map(|t| t.attributes.name.get("en").cloned())
            .collect();

        let mut author = None;
        let mut artist = None;
        let mut thumbnail_url = None;

        for rel in data.relationships {
            if let Some(attrs) = rel.attributes {
                match rel.rel_type.as_str() {
                    "author" => {
                        author = attrs.get("name").and_then(|v| v.as_str()).map(|s| s.to_string());
                    }
                    "artist" => {
                        artist = attrs.get("name").and_then(|v| v.as_str()).map(|s| s.to_string());
                    }
                    "cover_art" => {
                        if let Some(file_name) = attrs.get("fileName").and_then(|v| v.as_str()) {
                            thumbnail_url = Some(format!(
                                "https://uploads.mangadex.org/covers/{}/{}.256.jpg",
                                data.id, file_name
                            ));
                        }
                    }
                    _ => {}
                }
            }
        }

        SManga {
            url: format!("/manga/{}", data.id),
            title,
            author,
            artist,
            description,
            genre,
            status,
            thumbnail_url,
            update_strategy: UpdateStrategy::AlwaysUpdate,
            initialized: true,
        }
    }

    pub fn parse_manga_list_json(&self, json_str: &str) -> Result<MangasPage> {
        let parsed: MdMangaListResponse =
            serde_json::from_str(json_str).map_err(|e| IkazuchiError::Parse(e.to_string()))?;
        let has_next_page = parsed.offset + parsed.limit < parsed.total;
        let mangas = parsed.data.into_iter().map(|d| self.map_md_manga(d)).collect();
        Ok(MangasPage { mangas, has_next_page })
    }

    pub fn parse_chapter_list_json(&self, json_str: &str) -> Result<Vec<SChapter>> {
        let parsed: MdChapterListResponse =
            serde_json::from_str(json_str).map_err(|e| IkazuchiError::Parse(e.to_string()))?;
        let mut chapters = Vec::with_capacity(parsed.data.len());
        for ch in parsed.data {
            let ch_num_str = ch.attributes.chapter.as_deref().unwrap_or("-1");
            let chapter_number = ch_num_str.parse::<f32>().unwrap_or(-1.0);
            let mut name_parts = Vec::new();
            if let Some(vol) = &ch.attributes.volume {
                if !vol.is_empty() {
                    name_parts.push(format!("Vol.{vol}"));
                }
            }
            if let Some(num) = &ch.attributes.chapter {
                if !num.is_empty() {
                    name_parts.push(format!("Ch.{num}"));
                }
            }
            if let Some(title) = &ch.attributes.title {
                if !title.is_empty() {
                    if !name_parts.is_empty() {
                        name_parts.push("-".to_string());
                    }
                    name_parts.push(title.clone());
                }
            }
            let name = if name_parts.is_empty() {
                "Oneshot".to_string()
            } else {
                name_parts.join(" ")
            };

            let scanlator = ch.relationships.into_iter().find_map(|rel| {
                if rel.rel_type == "scanlation_group" {
                    rel.attributes?.get("name")?.as_str().map(|s| s.to_string())
                } else {
                    None
                }
            });

            chapters.push(SChapter {
                url: format!("/chapter/{}", ch.id),
                name,
                date_upload: 0,
                chapter_number,
                scanlator,
            });
        }
        Ok(chapters)
    }

    pub fn parse_at_home_pages_json(&self, json_str: &str) -> Result<Vec<Page>> {
        let parsed: MdAtHomeResponse =
            serde_json::from_str(json_str).map_err(|e| IkazuchiError::Parse(e.to_string()))?;
        let pages = parsed
            .chapter
            .data
            .into_iter()
            .enumerate()
            .map(|(idx, filename)| Page {
                index: idx as u32,
                url: String::new(),
                image_url: Some(format!("{}/data/{}/{}", parsed.base_url, parsed.chapter.hash, filename)),
            })
            .collect();
        Ok(pages)
    }
}

#[async_trait]
impl MangaSource for MangaDexSource {
    fn id(&self) -> i64 {
        self.id
    }
    fn name(&self) -> &str {
        "MangaDex"
    }
    fn lang(&self) -> &str {
        &self.lang
    }
    fn base_url(&self) -> &str {
        &self.base_url
    }

    async fn get_popular_manga(&self, page: u32) -> Result<MangasPage> {
        let offset = page.saturating_sub(1) * 20;
        let url = format!(
            "{}/manga?limit=20&offset={offset}&includes[]=cover_art&includes[]=author&includes[]=artist&order[followedCount]=desc&availableTranslatedLanguage[]={}",
            self.api_url, self.lang
        );
        self.parse_manga_list_json(&fetch_text(&self.client, &url).await?)
    }

    async fn get_latest_updates(&self, page: u32) -> Result<MangasPage> {
        let offset = page.saturating_sub(1) * 20;
        let url = format!(
            "{}/manga?limit=20&offset={offset}&includes[]=cover_art&order[latestUploadedChapter]=desc&availableTranslatedLanguage[]={}",
            self.api_url, self.lang
        );
        self.parse_manga_list_json(&fetch_text(&self.client, &url).await?)
    }

    async fn get_search_manga(&self, page: u32, query: &str, _filters: &FilterList) -> Result<MangasPage> {
        let offset = page.saturating_sub(1) * 20;
        let url = format!(
            "{}/manga?limit=20&offset={offset}&title={query}&includes[]=cover_art&includes[]=author&includes[]=artist",
            self.api_url
        );
        self.parse_manga_list_json(&fetch_text(&self.client, &url).await?)
    }

    async fn get_manga_details(&self, manga: &SManga) -> Result<SManga> {
        let manga_id = manga.url.trim_start_matches("/manga/");
        let url = format!(
            "{}/manga/{manga_id}?includes[]=cover_art&includes[]=author&includes[]=artist",
            self.api_url
        );
        let resp: MdSingleMangaResponse = serde_json::from_str(&fetch_text(&self.client, &url).await?)
            .map_err(|e| IkazuchiError::Parse(e.to_string()))?;
        Ok(self.map_md_manga(resp.data))
    }

    async fn get_chapter_list(&self, manga: &SManga) -> Result<Vec<SChapter>> {
        let manga_id = manga.url.trim_start_matches("/manga/");
        let url = format!(
            "{}/manga/{manga_id}/feed?limit=500&translatedLanguage[]={}&includes[]=scanlation_group&order[chapter]=desc",
            self.api_url, self.lang
        );
        self.parse_chapter_list_json(&fetch_text(&self.client, &url).await?)
    }

    async fn get_page_list(&self, chapter: &SChapter) -> Result<Vec<Page>> {
        let chapter_id = chapter.url.trim_start_matches("/chapter/");
        let url = format!("{}/at-home/server/{chapter_id}", self.api_url);
        self.parse_at_home_pages_json(&fetch_text(&self.client, &url).await?)
    }
}
