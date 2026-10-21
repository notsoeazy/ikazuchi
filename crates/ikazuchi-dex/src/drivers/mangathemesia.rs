//! # MangaThemesia / MangaStream Multisrc Driver (`MangaThemesiaSource`)
//!
//! Native Rust implementation of `eu.kanade.tachiyomi.multisrc.mangathemesia.MangaThemesia`
//! and `MangaStream`. Powers a huge share of manhwa/webtoon scanlation extensions.
//! Supports both `#readerarea img` DOM extraction and `ts_reader.run({...})` script JSON extraction!

use crate::chapter_num::parse_chapter_number;
use crate::drivers::{default_scraper_client, fetch_text, image_from_element};
use async_trait::async_trait;
use ikazuchi_core::{
    FilterList, MangaSource, MangaStatus, MangasPage, Page, Result, SChapter, SManga, UpdateStrategy,
    generate_source_id, url_without_domain,
};
use reqwest::Client;
use scraper::{Html, Selector};
use serde::Deserialize;

pub struct MangaThemesiaSource {
    pub id: i64,
    pub name: String,
    pub lang: String,
    pub base_url: String,
    pub manga_url_directory: String,
    pub client: Client,
}

#[derive(Deserialize)]
struct TsReaderPayload {
    sources: Vec<TsReaderSource>,
}

#[derive(Deserialize)]
struct TsReaderSource {
    images: Vec<String>,
}

impl MangaThemesiaSource {
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
            manga_url_directory: "/manga".to_string(),
            client: default_scraper_client(),
        }
    }

    /// Parses a MangaThemesia catalog/search HTML page (`.utao .uta .imgu, .listupd .bs .bsx`).
    pub fn parse_catalog_html(&self, html: &str) -> Result<MangasPage> {
        let doc = Html::parse_document(html);
        let item_sel = Selector::parse(".utao .uta .imgu, .listupd .bs .bsx, .listo .bs .bsx").unwrap();
        let a_sel = Selector::parse("a").unwrap();
        let img_sel = Selector::parse("img").unwrap();
        let title_sel = Selector::parse("div.tt, .ntitle").unwrap();
        let next_sel = Selector::parse("div.hpage a.r, .pagination a.next").unwrap();

        let mut mangas = Vec::new();
        for item in doc.select(&item_sel) {
            if let Some(a) = item.select(&a_sel).next() {
                let href = a.value().attr("href").unwrap_or_default();
                let title = a
                    .value()
                    .attr("title")
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .or_else(|| {
                        item.select(&title_sel)
                            .next()
                            .map(|t| t.text().collect::<String>().trim().to_string())
                    })
                    .unwrap_or_default();

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

    /// Parses MangaThemesia manga details HTML (`h1.entry-title`, `.seriestugenre a`, `.entry-content[itemprop=description]`).
    pub fn parse_details_html(&self, manga: &SManga, html: &str) -> Result<SManga> {
        let doc = Html::parse_document(html);
        let title_sel = Selector::parse("h1.entry-title, .ts-breadcrumb li:last-child span").unwrap();
        let desc_sel = Selector::parse("div.entry-content[itemprop=description], .desc").unwrap();
        let genre_sel = Selector::parse("div.gnr a, .mgen a, .seriestugenre a").unwrap();
        let thumb_sel = Selector::parse(".infomanga > div[itemprop=image] img, .thumb img").unwrap();
        let info_sel = Selector::parse(".tsinfo .imptdt, .infotable tr, .fmed").unwrap();

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

        let genre: Vec<String> = doc
            .select(&genre_sel)
            .map(|e| e.text().collect::<String>().trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        let mut author = manga.author.clone();
        let mut artist = manga.artist.clone();
        let mut status = MangaStatus::Unknown;

        for info in doc.select(&info_sel) {
            let text = info.text().collect::<String>();
            let lower = text.to_lowercase();
            if lower.contains("author") {
                let val = text
                    .split_once("Author")
                    .or_else(|| text.split_once("author"))
                    .map(|(_, v)| v.trim().to_string())
                    .unwrap_or_default();
                if !val.is_empty() && val != "-" {
                    author = Some(val);
                }
            } else if lower.contains("artist") {
                let val = text
                    .split_once("Artist")
                    .or_else(|| text.split_once("artist"))
                    .map(|(_, v)| v.trim().to_string())
                    .unwrap_or_default();
                if !val.is_empty() && val != "-" {
                    artist = Some(val);
                }
            } else if lower.contains("status") {
                if lower.contains("ongoing") {
                    status = MangaStatus::Ongoing;
                } else if lower.contains("completed") {
                    status = MangaStatus::Completed;
                } else if lower.contains("hiatus") {
                    status = MangaStatus::OnHiatus;
                } else if lower.contains("dropped") || lower.contains("cancelled") {
                    status = MangaStatus::Cancelled;
                }
            }
        }

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

    /// Parses MangaThemesia chapter list (`#chapterlist li, .bxcl li`).
    pub fn parse_chapters_html(&self, manga_title: &str, html: &str) -> Result<Vec<SChapter>> {
        let doc = Html::parse_document(html);
        let li_sel = Selector::parse("div.bxcl li, div.cl li, #chapterlist li, ul li:has(div.chbox)").unwrap();
        let a_sel = Selector::parse("a").unwrap();
        let num_sel = Selector::parse("span.chapternum").unwrap();

        let mut chapters = Vec::new();
        for li in doc.select(&li_sel) {
            if let Some(a) = li.select(&a_sel).next() {
                let href = a.value().attr("href").unwrap_or_default();
                if href.is_empty() {
                    continue;
                }
                let name = li
                    .select(&num_sel)
                    .next()
                    .map(|s| s.text().collect::<String>().trim().to_string())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| a.text().collect::<String>().trim().to_string());

                let chapter_number = li
                    .value()
                    .attr("data-num")
                    .and_then(|n| n.parse::<f32>().ok())
                    .unwrap_or_else(|| parse_chapter_number(&name, manga_title));

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

    /// Parses MangaThemesia reader pages from either `ts_reader.run({...})` JSON or `#readerarea img`.
    pub fn parse_pages_html(&self, html: &str) -> Result<Vec<Page>> {
        // 1. Check for `ts_reader.run({...})` script payload first (just like Mihon's MangaThemesia.kt!)
        if let Some(start) = html.find("ts_reader.run(") {
            let after = &html[start + "ts_reader.run(".len()..];
            if let Some(end) = after.find(");") {
                let json_str = after[..end].trim();
                if let Ok(payload) = serde_json::from_str::<TsReaderPayload>(json_str) {
                    if let Some(first_source) = payload.sources.into_iter().next() {
                        let pages: Vec<Page> = first_source
                            .images
                            .into_iter()
                            .enumerate()
                            .map(|(idx, img_url)| Page {
                                index: idx as u32,
                                url: String::new(),
                                image_url: Some(img_url),
                            })
                            .collect();
                        if !pages.is_empty() {
                            return Ok(pages);
                        }
                    }
                }
            }
        }

        // 2. Fallback: `#readerarea img` DOM selector
        let doc = Html::parse_document(html);
        let img_sel = Selector::parse("div#readerarea img").unwrap();
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
impl MangaSource for MangaThemesiaSource {
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
        let url = format!("{}{}/?page={page}&order=popular", self.base_url, self.manga_url_directory);
        self.parse_catalog_html(&fetch_text(&self.client, &url).await?)
    }

    async fn get_latest_updates(&self, page: u32) -> Result<MangasPage> {
        let url = format!("{}{}/?page={page}&order=update", self.base_url, self.manga_url_directory);
        self.parse_catalog_html(&fetch_text(&self.client, &url).await?)
    }

    async fn get_search_manga(&self, page: u32, query: &str, _filters: &FilterList) -> Result<MangasPage> {
        let url = format!("{}/page/{page}/?s={query}", self.base_url);
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
