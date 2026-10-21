//! # `ikazuchi://` Custom Streaming Asset Protocol
//!
//! Parses and resolves `ikazuchi://localhost/...` image requests from the Dioxus renderer:
//! - `ikazuchi://localhost/cbz/<page_index>?path=<cbz_path>` -> Streams page directly from `.cbz` archive (`ikazuchi-archive`).
//! - `ikazuchi://localhost/cover/<source_id>?url=<image_url>` -> Streams cover thumbnail with source headers.
//! - `ikazuchi://localhost/page/<source_id>/<chapter_id>/<page_index>?url=<image_url>` -> Streams chapter page.

use ikazuchi_archive::{detect_image_mime, read_cbz_page_bytes};
use ikazuchi_core::{IkazuchiError, Result};

/// Parsed target of an `ikazuchi://` URI request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IkazuchiAssetRoute {
    /// Stream a page directly from a local `.cbz` archive.
    LocalCbzPage { cbz_path: String, page_index: usize },
    /// Fetch and cache a manga cover thumbnail using the source's HTTP client.
    RemoteCover { source_id: i64, image_url: String },
    /// Fetch and cache a chapter page image using the source's HTTP client.
    RemotePage {
        source_id: i64,
        chapter_id: i64,
        page_index: usize,
        image_url: String,
    },
}

/// Parses an `ikazuchi://localhost/...` URI string into an [`IkazuchiAssetRoute`].
pub fn parse_ikazuchi_uri(uri_str: &str) -> Result<IkazuchiAssetRoute> {
    let after_scheme = uri_str
        .strip_prefix("ikazuchi://")
        .ok_or_else(|| IkazuchiError::Parse(format!("Invalid ikazuchi URI scheme: {uri_str}")))?;
    let path_and_query = after_scheme
        .strip_prefix("localhost")
        .unwrap_or(after_scheme);
    let (path_part, query_part) = path_and_query.split_once('?').unwrap_or((path_and_query, ""));
    let segments: Vec<&str> = path_part.split('/').filter(|s| !s.is_empty()).collect();

    let get_query_param = |key: &str| -> Option<String> {
        query_part.split('&').find_map(|pair| {
            let (k, v) = pair.split_once('=')?;
            (k == key).then(|| v.replace("%20", " "))
        })
    };

    match segments.as_slice() {
        ["cbz", page_idx_str] => {
            let page_index = page_idx_str
                .parse::<usize>()
                .map_err(|_| IkazuchiError::Parse("Invalid cbz page index".into()))?;
            let cbz_path = get_query_param("path")
                .ok_or_else(|| IkazuchiError::Parse("Missing ?path= query on cbz route".into()))?;
            Ok(IkazuchiAssetRoute::LocalCbzPage { cbz_path, page_index })
        }
        ["cover", source_id_str] => {
            let source_id = source_id_str
                .parse::<i64>()
                .map_err(|_| IkazuchiError::Parse("Invalid source_id".into()))?;
            let image_url = get_query_param("url")
                .ok_or_else(|| IkazuchiError::Parse("Missing ?url= query on cover route".into()))?;
            Ok(IkazuchiAssetRoute::RemoteCover { source_id, image_url })
        }
        ["page", source_id_str, chapter_id_str, page_idx_str] => {
            let source_id = source_id_str
                .parse::<i64>()
                .map_err(|_| IkazuchiError::Parse("Invalid source_id".into()))?;
            let chapter_id = chapter_id_str
                .parse::<i64>()
                .map_err(|_| IkazuchiError::Parse("Invalid chapter_id".into()))?;
            let page_index = page_idx_str
                .parse::<usize>()
                .map_err(|_| IkazuchiError::Parse("Invalid page_index".into()))?;
            let image_url = get_query_param("url")
                .ok_or_else(|| IkazuchiError::Parse("Missing ?url= query on page route".into()))?;
            Ok(IkazuchiAssetRoute::RemotePage {
                source_id,
                chapter_id,
                page_index,
                image_url,
            })
        }
        _ => Err(IkazuchiError::Parse(format!("Unrecognized ikazuchi URI path: {uri_str}"))),
    }
}

/// Resolves an [`IkazuchiAssetRoute`] into `(content_type, raw_bytes)`.
pub async fn resolve_ikazuchi_asset(route: &IkazuchiAssetRoute) -> Result<(&'static str, Vec<u8>)> {
    match route {
        IkazuchiAssetRoute::LocalCbzPage { cbz_path, page_index } => {
            let bytes = read_cbz_page_bytes(cbz_path, *page_index)?;
            Ok((detect_image_mime(&bytes), bytes))
        }
        IkazuchiAssetRoute::RemoteCover { image_url, .. } | IkazuchiAssetRoute::RemotePage { image_url, .. } => {
            let bytes = reqwest::get(image_url)
                .await
                .map_err(|e| IkazuchiError::Network(e.to_string()))?
                .error_for_status()
                .map_err(|e| IkazuchiError::Network(e.to_string()))?
                .bytes()
                .await
                .map_err(|e| IkazuchiError::Network(e.to_string()))?
                .to_vec();
            Ok((detect_image_mime(&bytes), bytes))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ikazuchi_archive::write_cbz_archive;

    #[tokio::test]
    async fn test_parse_and_stream_cbz_via_ikazuchi_protocol() {
        let cbz_file = std::env::temp_dir().join("ikazuchi_protocol_test.cbz");
        let fake_png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 1, 2, 3];
        write_cbz_archive(&cbz_file, &[("001.png".to_string(), fake_png.clone())]).unwrap();

        let uri = format!("ikazuchi://localhost/cbz/0?path={}", cbz_file.display());
        let route = parse_ikazuchi_uri(&uri).unwrap();
        assert_eq!(
            route,
            IkazuchiAssetRoute::LocalCbzPage {
                cbz_path: cbz_file.display().to_string(),
                page_index: 0
            }
        );

        let (mime, bytes) = resolve_ikazuchi_asset(&route).await.unwrap();
        assert_eq!(mime, "image/png");
        assert_eq!(bytes, fake_png);

        // Test out-of-bounds page index error handling
        let out_of_bounds_route = IkazuchiAssetRoute::LocalCbzPage {
            cbz_path: cbz_file.display().to_string(),
            page_index: 99,
        };
        assert!(resolve_ikazuchi_asset(&out_of_bounds_route).await.is_err());

        // Test invalid protocol scheme or malformed URI paths
        assert!(parse_ikazuchi_uri("https://localhost/cbz/0").is_err());
        assert!(parse_ikazuchi_uri("ikazuchi://localhost/unknown_route").is_err());
        assert!(parse_ikazuchi_uri("ikazuchi://localhost/cbz/not_a_number?path=/tmp/test.cbz").is_err());
        assert!(parse_ikazuchi_uri("ikazuchi://localhost/cbz/0").is_err()); // Missing ?path=

        // Test valid cover & page URI parsing
        let cover_route = parse_ikazuchi_uri("ikazuchi://localhost/cover/12345?url=https://example.org/cover.jpg").unwrap();
        assert_eq!(
            cover_route,
            IkazuchiAssetRoute::RemoteCover {
                source_id: 12345,
                image_url: "https://example.org/cover.jpg".into(),
            }
        );

        let page_route = parse_ikazuchi_uri("ikazuchi://localhost/page/123/456/7?url=https://example.org/p7.webp").unwrap();
        assert_eq!(
            page_route,
            IkazuchiAssetRoute::RemotePage {
                source_id: 123,
                chapter_id: 456,
                page_index: 7,
                image_url: "https://example.org/p7.webp".into(),
            }
        );

        let _ = std::fs::remove_file(&cbz_file);
    }
}
