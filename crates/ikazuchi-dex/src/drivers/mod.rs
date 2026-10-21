//! # Native Rust `MangaSource` Drivers
//!
//! High-performance pure-Rust scrapers for the most common Mihon Multisrc themes and APIs:
//! - [`mangadex::MangaDexSource`] — MangaDex API v5
//! - [`madara::MadaraSource`] — WordPress Madara theme
//! - [`mangathemesia::MangaThemesiaSource`] — MangaThemesia / MangaStream theme
//! - [`mangabox::MangaboxSource`] — Mangabox / Mangakakalot / Manganelo theme

pub mod local;
pub mod madara;
pub mod mangabox;
pub mod mangadex;
pub mod mangathemesia;

use ikazuchi_core::{IkazuchiError, Result};
use reqwest::Client;
use scraper::ElementRef;

pub(crate) fn default_scraper_client() -> Client {
    Client::builder()
        .user_agent("Mozilla/5.0 (X11; Linux x86_64; rv:128.0) Gecko/20100101 Firefox/128.0")
        .cookie_store(true)
        .build()
        .unwrap_or_default()
}

pub(crate) async fn fetch_text(client: &Client, url: &str) -> Result<String> {
    client
        .get(url)
        .send()
        .await
        .map_err(|e| IkazuchiError::Network(e.to_string()))?
        .error_for_status()
        .map_err(|e| IkazuchiError::Network(e.to_string()))?
        .text()
        .await
        .map_err(|e| IkazuchiError::Network(e.to_string()))
}

/// Extracts an image URL from an `<img>` element, checking lazy-loading attributes first
/// (`data-src`, `data-lazy-src`, `data-cfsrc`, `srcset`, `src`) just like Mihon's `imageFromElement`.
pub(crate) fn image_from_element(el: &ElementRef<'_>) -> Option<String> {
    let v = el.value();
    for attr in ["data-src", "data-lazy-src", "data-cfsrc", "src"] {
        if let Some(val) = v.attr(attr) {
            let trimmed = val.trim();
            if !trimmed.is_empty() && !trimmed.starts_with("data:image") {
                return Some(trimmed.to_string());
            }
        }
    }
    if let Some(srcset) = v.attr("srcset") {
        if let Some(first_url) = srcset.split(',').next().and_then(|s| s.split_whitespace().next()) {
            return Some(first_url.trim().to_string());
        }
    }
    None
}
