//! # Mangabox Multisrc Driver (`MangaboxSource`)
//!
//! Native Rust implementation of `eu.kanade.tachiyomi.multisrc.mangabox.Mangabox`
//! (Mangakakalot, Manganelo,anganato, etc.).

use crate::chapter_num::parse_chapter_number;
use crate::drivers::{default_scraper_client, fetch_text, image_from_element};
use async_trait::async_trait;
use ikazuchi_core::{
    FilterList, MangaSource, MangaStatus, MangasPage, Page, Result, SChapter, SManga, UpdateStrategy,
    generate_source_id, url_without_domain,
};
use reqwest::Client;
use scraper::{Html, Selector};

pub struct MangaboxSource {
    pub id: i64,
    pub name: String,
    pub lang: String,
    pub base_url: String,
    pub client: Client,
}

impl MangaboxSource {
    pub fn new(name: impl Into<String>, base_url: impl Into<String>, lang: impl Into<String>, version_id: i32) -> Self {
        let name = name.into();
        let lang = lang.into();
        let base_url = base_url.into().trim_end_matches('/').to_string();
        let id = generate_source_id(&name, &lang, version_id);

        Self {
            id,
            name,
            lang,
            base_url,
            client: default_scraper_client(),
        }
    }

    pub fn parse_catalog_html(&self, html: &str) -> Result<MangasPage> {
        let doc = Html::parse_document(html);
        let item_sel = Selector::parse("div.content-genres-item, div.list-truyen-item-wrap, div.search-story-item, div.story_item").unwrap();
        let link_sel = Selector::parse("h3 a, a.list-story-item").unwrap();
        let img_sel = Selector::parse("img").unwrap();
        let next_sel = Selector::parse("a.page-blue.page-last, a.page_select + a").unwrap();

        let mut mangas = Vec::new();
        for item in doc.select(&item_sel) {
            if let Some(a) = item.select(&link_sel).next() {
                let href = a.value().attr("href").unwrap_or_default();
                let title = a.text().collect::<String>().trim().to_string();
                if href.is_empty() || title.is_empty() {
                    continue;
                }
                let thumbnail_url = item.select(&img_sel).next().and_then(|img| image_from_element(&img));
                mangas.push(SManga {
                    url: url_without_domain(href),
                    title,
                    thumbnail_url,
                    ..Default::default()
                });
            }
        }
        let has_next_page = doc.select(&next_sel).next().is_some();
        Ok(MangasPage { mangas, has_next_page })
    }

    pub fn parse_details_html(&self, manga: &SManga, html: &str) -> Result<SManga> {
        let doc = Html::parse_document(html);
        let title_sel = Selector::parse("div.story-info-right h1, ul.manga-info-text h1").unwrap();
        let desc_sel = Selector::parse("div#panel-story-info-description, div#noidungm").unwrap();
        let thumb_sel = Selector::parse("div.story-info-left span.info-image img, div.manga-info-pic img").unwrap();

        let title = doc
            .select(&title_sel)
            .next()
            .map(|e| e.text().collect::<String>().trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| manga.title.clone());

        let description = doc
            .select(&desc_sel)
            .next()
            .map(|e| e.text().collect::<String>().trim().to_string())
            .filter(|s| !s.is_empty())
            .or_else(|| manga.description.clone());

        let thumbnail_url = doc
            .select(&thumb_sel)
            .next()
            .and_then(|img| img.value().attr("src").map(|s| s.to_string()))
            .or_else(|| manga.thumbnail_url.clone());

        Ok(SManga {
            url: manga.url.clone(),
            title,
            author: manga.author.clone(),
            artist: manga.artist.clone(),
            description,
            genre: manga.genre.clone(),
            status: MangaStatus::Ongoing,
            thumbnail_url,
            update_strategy: UpdateStrategy::AlwaysUpdate,
            initialized: true,
        })
    }

    pub fn parse_chapters_html(&self, manga_title: &str, html: &str) -> Result<Vec<SChapter>> {
        let doc = Html::parse_document(html);
        let ch_sel = Selector::parse("div.chapter-list div.row, ul.row-content-chapter li").unwrap();
        let a_sel = Selector::parse("a").unwrap();

        let mut chapters = Vec::new();
        for row in doc.select(&ch_sel) {
            if let Some(a) = row.select(&a_sel).next() {
                let href = a.value().attr("href").unwrap_or_default();
                let name = a.text().collect::<String>().trim().to_string();
                if href.is_empty() || name.is_empty() {
                    continue;
                }
                let chapter_number = parse_chapter_number(&name, manga_title);
                chapters.push(SChapter {
                    url: url_without_domain(href),
                    name,
                    date_upload: 0,
                    chapter_number,
                    scanlator: None,
                });
            }
        }
        Ok(chapters)
    }

    pub fn parse_pages_html(&self, html: &str) -> Result<Vec<Page>> {
        let doc = Html::parse_document(html);
        let img_sel = Selector::parse("div.container-chapter-reader img, div#vungdoc img").unwrap();
        let mut pages = Vec::new();
        for (idx, img) in doc.select(&img_sel).enumerate() {
            if let Some(src) = img.value().attr("data-src").or_else(|| img.value().attr("src")) {
                pages.push(Page {
                    index: idx as u32,
                    url: String::new(),
                    image_url: Some(src.to_string()),
                });
            }
        }
        Ok(pages)
    }
}

#[async_trait]
impl MangaSource for MangaboxSource {
    fn id(&self) -> i64 {
        self.id
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn lang(&self) -> &str {
        &self.lang
    }
    fn base_url(&self) -> &str {
        &self.base_url
    }

    async fn get_popular_manga(&self, page: u32) -> Result<MangasPage> {
        let url = format!("{}/manga_list?type=topview&category=all&state=all&page={page}", self.base_url);
        self.parse_catalog_html(&fetch_text(&self.client, &url).await?)
    }

    async fn get_latest_updates(&self, page: u32) -> Result<MangasPage> {
        let url = format!("{}/manga_list?type=latest&category=all&state=all&page={page}", self.base_url);
        self.parse_catalog_html(&fetch_text(&self.client, &url).await?)
    }

    async fn get_search_manga(&self, page: u32, query: &str, _filters: &FilterList) -> Result<MangasPage> {
        let normalized = query.trim().replace(' ', "_");
        let url = format!("{}/search/story/{normalized}?page={page}", self.base_url);
        self.parse_catalog_html(&fetch_text(&self.client, &url).await?)
    }

    async fn get_manga_details(&self, manga: &SManga) -> Result<SManga> {
        let url = format!("{}{}", self.base_url, manga.url);
        self.parse_details_html(manga, &fetch_text(&self.client, &url).await?)
    }

    async fn get_chapter_list(&self, manga: &SManga) -> Result<Vec<SChapter>> {
        let url = format!("{}{}", self.base_url, manga.url);
        self.parse_chapters_html(&manga.title, &fetch_text(&self.client, &url).await?)
    }

    async fn get_page_list(&self, chapter: &SChapter) -> Result<Vec<Page>> {
        let url = format!("{}{}", self.base_url, chapter.url);
        self.parse_pages_html(&fetch_text(&self.client, &url).await?)
    }
}
