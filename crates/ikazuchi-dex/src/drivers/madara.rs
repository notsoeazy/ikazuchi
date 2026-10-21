//! # WordPress Madara Multisrc Driver (`MadaraSource`)
//!
//! Native Rust implementation of `eu.kanade.tachiyomi.multisrc.madara.Madara`.
//! Powers hundreds of Mihon manga/manhwa extensions.

use crate::chapter_num::parse_chapter_number;
use crate::drivers::{default_scraper_client, fetch_text, image_from_element};
use async_trait::async_trait;
use ikazuchi_core::{
    FilterList, MangaSource, MangaStatus, MangasPage, Page, Result, SChapter, SManga, UpdateStrategy,
    generate_source_id, url_without_domain,
};
use reqwest::Client;
use scraper::{Html, Selector};

pub struct MadaraSource {
    pub id: i64,
    pub name: String,
    pub lang: String,
    pub base_url: String,
    pub client: Client,
}

impl MadaraSource {
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

    /// Parses a Madara catalog/search HTML page (`div.page-item-detail` or `div.c-tabs-item__content`).
    pub fn parse_catalog_html(&self, html: &str) -> Result<MangasPage> {
        let doc = Html::parse_document(html);
        let item_sel = Selector::parse("div.page-item-detail, div.c-tabs-item__content, .manga__item").unwrap();
        let title_link_sel = Selector::parse("div.post-title a, h3 a, h5 a").unwrap();
        let img_sel = Selector::parse("img").unwrap();
        let next_sel = Selector::parse("div.nav-previous, a.nextpostslink, a.next.page-numbers").unwrap();

        let mut mangas = Vec::new();
        for item in doc.select(&item_sel) {
            if let Some(link) = item.select(&title_link_sel).next() {
                let href = link.value().attr("href").unwrap_or_default();
                let title = link.text().collect::<String>().trim().to_string();
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

    /// Parses a Madara manga details HTML page.
    pub fn parse_details_html(&self, manga: &SManga, html: &str) -> Result<SManga> {
        let doc = Html::parse_document(html);
        let title_sel = Selector::parse("div.post-title h1, div.post-title h3").unwrap();
        let author_sel = Selector::parse("div.author-content a").unwrap();
        let artist_sel = Selector::parse("div.artist-content a").unwrap();
        let desc_sel =
            Selector::parse("div.description-summary div.summary__content, div.summary__content, div.manga-excerpt")
                .unwrap();
        let genre_sel = Selector::parse("div.genres-content a").unwrap();
        let status_sel = Selector::parse("div.post-status div.summary-content, div.post-content_item").unwrap();
        let thumb_sel = Selector::parse("div.summary_image img").unwrap();

        let title = doc
            .select(&title_sel)
            .next()
            .map(|el| el.text().collect::<String>().trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| manga.title.clone());

        let author = {
            let list: Vec<String> = doc
                .select(&author_sel)
                .map(|e| e.text().collect::<String>().trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            if list.is_empty() {
                manga.author.clone()
            } else {
                Some(list.join(", "))
            }
        };

        let artist = {
            let list: Vec<String> = doc
                .select(&artist_sel)
                .map(|e| e.text().collect::<String>().trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            if list.is_empty() {
                manga.artist.clone()
            } else {
                Some(list.join(", "))
            }
        };

        let description = doc
            .select(&desc_sel)
            .next()
            .map(|e| e.text().collect::<String>().trim().to_string())
            .filter(|s| !s.is_empty())
            .or_else(|| manga.description.clone());

        let genre: Vec<String> = doc
            .select(&genre_sel)
            .map(|e| e.text().collect::<String>().trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        let status_text = doc
            .select(&status_sel)
            .map(|e| e.text().collect::<String>().to_lowercase())
            .collect::<Vec<_>>()
            .join(" ");

        let status = if status_text.contains("completed") || status_text.contains("completo") {
            MangaStatus::Completed
        } else if status_text.contains("ongoing") || status_text.contains("updating") {
            MangaStatus::Ongoing
        } else if status_text.contains("hiatus") || status_text.contains("on hold") {
            MangaStatus::OnHiatus
        } else if status_text.contains("canceled") || status_text.contains("cancelled") || status_text.contains("dropped") {
            MangaStatus::Cancelled
        } else {
            MangaStatus::Unknown
        };

        let thumbnail_url = doc
            .select(&thumb_sel)
            .next()
            .and_then(|img| image_from_element(&img))
            .or_else(|| manga.thumbnail_url.clone());

        Ok(SManga {
            url: manga.url.clone(),
            title,
            author,
            artist,
            description,
            genre,
            status,
            thumbnail_url,
            update_strategy: UpdateStrategy::AlwaysUpdate,
            initialized: true,
        })
    }

    /// Parses a Madara chapter list (`li.wp-manga-chapter`).
    pub fn parse_chapters_html(&self, manga_title: &str, html: &str) -> Result<Vec<SChapter>> {
        let doc = Html::parse_document(html);
        let ch_sel = Selector::parse("li.wp-manga-chapter").unwrap();
        let a_sel = Selector::parse("a").unwrap();

        let mut chapters = Vec::new();
        for li in doc.select(&ch_sel) {
            if let Some(a) = li.select(&a_sel).next() {
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

    /// Parses a Madara chapter reader page (`div.reading-content img, .page-break img`).
    pub fn parse_pages_html(&self, html: &str) -> Result<Vec<Page>> {
        let doc = Html::parse_document(html);
        let img_sel = Selector::parse("div.reading-content div.page-break img, div.reading-content img, .wp-manga-chapter-img").unwrap();

        let mut pages = Vec::new();
        for (idx, img) in doc.select(&img_sel).enumerate() {
            if let Some(image_url) = image_from_element(&img) {
                pages.push(Page {
                    index: idx as u32,
                    url: String::new(),
                    image_url: Some(image_url),
                });
            }
        }
        Ok(pages)
    }
}

#[async_trait]
impl MangaSource for MadaraSource {
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
        let page_segment = if page > 1 { format!("page/{page}/") } else { String::new() };
        let url = format!("{}/manga/{}?m_orderby=views", self.base_url, page_segment);
        self.parse_catalog_html(&fetch_text(&self.client, &url).await?)
    }

    async fn get_latest_updates(&self, page: u32) -> Result<MangasPage> {
        let page_segment = if page > 1 { format!("page/{page}/") } else { String::new() };
        let url = format!("{}/manga/{}?m_orderby=latest", self.base_url, page_segment);
        self.parse_catalog_html(&fetch_text(&self.client, &url).await?)
    }

    async fn get_search_manga(&self, page: u32, query: &str, _filters: &FilterList) -> Result<MangasPage> {
        let page_segment = if page > 1 { format!("page/{page}/") } else { String::new() };
        let url = format!("{}/{page_segment}?s={query}&post_type=wp-manga", self.base_url);
        self.parse_catalog_html(&fetch_text(&self.client, &url).await?)
    }

    async fn get_manga_details(&self, manga: &SManga) -> Result<SManga> {
        let url = format!("{}{}", self.base_url, manga.url);
        self.parse_details_html(manga, &fetch_text(&self.client, &url).await?)
    }

    async fn get_chapter_list(&self, manga: &SManga) -> Result<Vec<SChapter>> {
        let url = format!("{}{}", self.base_url, manga.url);
        let html = fetch_text(&self.client, &url).await?;
        let mut chapters = self.parse_chapters_html(&manga.title, &html)?;
        if chapters.is_empty() {
            let ajax_url = format!("{}{}ajax/chapters/", self.base_url, manga.url);
            if let Ok(resp) = self.client.post(&ajax_url).header("X-Requested-With", "XMLHttpRequest").send().await {
                if let Ok(ajax_html) = resp.text().await {
                    chapters = self.parse_chapters_html(&manga.title, &ajax_html)?;
                }
            }
        }
        Ok(chapters)
    }

    async fn get_page_list(&self, chapter: &SChapter) -> Result<Vec<Page>> {
        let url = format!("{}{}?style=list", self.base_url, chapter.url);
        self.parse_pages_html(&fetch_text(&self.client, &url).await?)
    }
}
