//! # Chapter Number Recognition (`ChapterRecognition` port from Mihon)
//!
//! Extracts a normalized `f32` chapter number from raw chapter titles returned by scrapers
//! (e.g., `"Vol.2 Ch.14.5: Extra"` -> `14.5`), while stripping out numbers that belong to
//! the manga's own title (e.g. `"86 - Eighty Six - Chapter 7"` -> `7.0`).

/// Parses a chapter number from `chapter_name`, stripping `manga_title` first.
/// Returns `-1.0` if no chapter number can be recognized (matching Mihon's `SChapter` default).
pub fn parse_chapter_number(chapter_name: &str, manga_title: &str) -> f32 {
    let mut clean = chapter_name.to_lowercase();
    let title_lower = manga_title.trim().to_lowercase();
    if !title_lower.is_empty() && clean.starts_with(&title_lower) {
        clean = clean[title_lower.len()..].to_string();
    }

    clean = clean.replace(',', ".");

    // 1. Look for explicit prefixes: "chapter 12.5", "ch. 12", "ch 12", "ep. 12", "episode 12", "c12"
    let prefixes = ["chapter", "chap", "ch.", "ch", "episode", "ep.", "ep", "capítulo", "cap"];
    for prefix in prefixes {
        if let Some(idx) = clean.find(prefix) {
            let after = &clean[idx + prefix.len()..];
            if let Some(num) = extract_first_number_after_prefix(after) {
                return num;
            }
        }
    }

    // 2. Remove volume prefixes like "vol. 2" or "v2" so volume numbers aren't mistaken for chapter numbers
    for vol_prefix in ["volume", "vol.", "vol"] {
        if let Some(idx) = clean.find(vol_prefix) {
            let after = &clean[idx + vol_prefix.len()..];
            let skip = after
                .find(|c: char| c.is_ascii_digit())
                .map(|start| {
                    let num_slice = &after[start..];
                    let end = num_slice
                        .find(|c: char| !c.is_ascii_digit() && c != '.')
                        .unwrap_or(num_slice.len());
                    idx + vol_prefix.len() + start + end
                })
                .unwrap_or(idx + vol_prefix.len());
            clean.replace_range(idx..skip, " ");
        }
    }

    // 3. Fallback: extract first standalone number in the remaining string
    extract_first_number(&clean).unwrap_or(-1.0)
}

fn extract_first_number_after_prefix(s: &str) -> Option<f32> {
    let trimmed = s.trim_start_matches(|c: char| c.is_whitespace() || c == ':' || c == '-' || c == '.' || c == '#');
    if !trimmed.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    extract_first_number(trimmed)
}

fn extract_first_number(s: &str) -> Option<f32> {
    let start = s.find(|c: char| c.is_ascii_digit())?;
    let rest = &s[start..];
    let mut end = 0;
    let mut seen_dot = false;
    for (i, ch) in rest.char_indices() {
        if ch.is_ascii_digit() {
            end = i + ch.len_utf8();
        } else if ch == '.' && !seen_dot {
            seen_dot = true;
            end = i + 1;
        } else {
            break;
        }
    }
    let num_str = rest[..end].trim_end_matches('.');
    let base: f32 = num_str.parse().ok()?;

    // Handle ".a", ".b", "extra" suffixes right after the number (e.g. "14a" -> 14.1)
    let suffix = rest[end..].chars().next();
    let fractional = match suffix {
        Some('a') => 0.1,
        Some('b') => 0.2,
        Some('c') => 0.3,
        _ => 0.0,
    };

    Some(base + fractional)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chapter_number_recognition() {
        assert_eq!(parse_chapter_number("Chapter 142: Thunder God", "Berserk"), 142.0);
        assert_eq!(parse_chapter_number("Vol. 4 Ch. 28.5 - Bonus", "Vagabond"), 28.5);
        assert_eq!(parse_chapter_number("86 - Eighty Six Chapter 19", "86 - Eighty Six"), 19.0);
        assert_eq!(parse_chapter_number("Episode 104", "Solo Leveling"), 104.0);
        assert_eq!(parse_chapter_number("Ch.15a", "Test"), 15.1);
    }
}
