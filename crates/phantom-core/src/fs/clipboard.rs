use std::collections::HashSet;
use std::path::Path;

pub const MAX_CLIPBOARD_PATHS: usize = 1024;
pub const MAX_CLIPBOARD_BYTES: usize = 1024 * 1024;

pub fn local_file_url(uri: &str) -> Option<reqwest::Url> {
    if uri.contains('\\') || uri.chars().any(char::is_control) {
        return None;
    }
    if !uri
        .get(..5)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("file:"))
        || !uri[5..].starts_with('/')
    {
        return None;
    }
    let url = reqwest::Url::parse(uri).ok()?;
    if url.query().is_some()
        || url.fragment().is_some()
        || url
            .host_str()
            .is_some_and(|host| !host.is_empty() && !host.eq_ignore_ascii_case("localhost"))
    {
        return None;
    }
    Some(url)
}

pub fn clipboard_path(path: impl AsRef<Path>) -> Result<String, String> {
    let path = path.as_ref();
    let text = path.to_str().ok_or("clipboard path is not valid Unicode")?;
    if !path.is_absolute() || text.contains('\0') {
        return Err("clipboard path must be absolute and contain no NUL bytes".into());
    }
    #[cfg(unix)]
    if text.contains('\\') {
        return Err("clipboard filenames containing backslashes are not supported".into());
    }
    Ok(super::to_canon(path))
}

pub fn clipboard_file_paths<'a>(
    uris: impl IntoIterator<Item = &'a str>,
) -> Result<Vec<String>, String> {
    let mut bytes = 0usize;
    let mut seen = HashSet::new();
    let mut paths = Vec::new();
    for raw in uris {
        let size = raw.len().saturating_add(1);
        if size > MAX_CLIPBOARD_BYTES.saturating_sub(bytes) {
            return Err("clipboard file list is too large".into());
        }
        bytes += size;
        let uri = raw.trim();
        let Some(url) = local_file_url(uri) else {
            continue;
        };
        let Ok(path) = url.to_file_path() else {
            continue;
        };
        let path = clipboard_path(path)?;
        if !seen.insert(path.clone()) {
            continue;
        }
        if paths.len() >= MAX_CLIPBOARD_PATHS {
            return Err(format!(
                "paste at most {MAX_CLIPBOARD_PATHS} files at a time"
            ));
        }
        paths.push(path);
    }
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn decodes_gnome_lists_and_deduplicates_without_losing_order() {
        let text = "copy\n# comment\nfile:///tmp/My%20File\r\nfile://localhost/tmp/My%20File\nfile:///tmp/%E4%B8%AD%E6%96%87\n";
        assert_eq!(
            clipboard_file_paths(text.lines()).unwrap(),
            ["/tmp/My File", "/tmp/中文"]
        );
        assert_eq!(
            clipboard_file_paths("cut\nfile:///tmp/a".lines()).unwrap(),
            ["/tmp/a"]
        );
    }

    #[test]
    fn refuses_remote_urls_and_does_not_interpret_plain_paths_as_files() {
        assert!(clipboard_file_paths([
            "file://remote-server/tmp/a",
            "https://example.com/a",
            "/tmp/a",
            "file:relative",
            "file:///tmp/a?query",
            "file:///tmp/a#fragment",
            "file:///tmp/a\\b",
            "file:///tmp/a\tb",
        ])
        .unwrap()
        .is_empty());
    }

    #[test]
    fn rejects_nul_and_relative_paths() {
        assert!(clipboard_path("relative").is_err());
        assert!(clipboard_path("/tmp/a\0b").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn never_turns_a_literal_backslash_into_a_different_source_path() {
        assert!(clipboard_path("/tmp/a\\b").is_err());
        assert!(clipboard_file_paths(["file:///tmp/a%5Cb"]).is_err());
        assert!(clipboard_file_paths(["file:///tmp/a%00b"]).is_err());
    }

    #[test]
    fn bounds_the_input_before_parsing_it() {
        let text = "x".repeat(MAX_CLIPBOARD_BYTES + 1);
        assert!(clipboard_file_paths([text.as_str()]).is_err());
        assert!(clipboard_file_paths(std::iter::repeat_n("", MAX_CLIPBOARD_BYTES + 1)).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn bounds_the_number_of_unique_sources() {
        let uris: Vec<_> = (0..=MAX_CLIPBOARD_PATHS)
            .map(|n| format!("file:///tmp/{n}"))
            .collect();
        assert!(clipboard_file_paths(uris.iter().map(String::as_str)).is_err());
    }
}
