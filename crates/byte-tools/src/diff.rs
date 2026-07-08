//! Diff generation utilities for file-editing tools.

use std::path::Path;

use similar::TextDiff;

/// Number of unchanged lines to include around each hunk.
const DEFAULT_CONTEXT: usize = 3;

/// Generate a unified diff between `old` and `new` content for `path`.
///
/// If `old` is `None`, the diff is treated as creating a new file. The output
/// follows the unified diff format so it can be parsed by the UI or read by
/// the model as a concise summary of changes.
///
/// # Panics
///
/// Panics only if the internal in-memory buffer write fails. This cannot happen
/// in practice because the buffer is a `Vec<u8>` and the diff is generated from
/// valid UTF-8 strings.
#[must_use]
pub fn unified_diff(path: &Path, old: Option<&str>, new: &str) -> String {
    let old_text = old.unwrap_or("");
    let diff = TextDiff::from_lines(old_text, new);

    let old_label = old.map_or_else(
        || "--- /dev/null".to_owned(),
        |_| format!("--- {}", path.display()),
    );
    let new_label = format!("+++ {}", path.display());

    let mut output = Vec::new();
    #[allow(clippy::expect_used)]
    {
        diff.unified_diff()
            .context_radius(DEFAULT_CONTEXT)
            .header(&old_label, &new_label)
            .to_writer(&mut output)
            .expect("writing to a Vec should never fail");
    }

    // `similar` writes a trailing newline; trim it to keep the summary + diff
    // formatting consistent with the rest of the tool output.
    let mut text = String::from_utf8_lossy(&output).into_owned();
    if text.ends_with('\n') {
        let _ = text.pop();
    }
    text
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used, unused_results)]

    use super::*;
    use std::path::PathBuf;

    #[test]
    fn new_file_diff_uses_null_old_file() {
        let diff = unified_diff(Path::new("hello.txt"), None, "Hello, world!\n");
        assert!(diff.contains("--- /dev/null"));
        assert!(diff.contains("+++ hello.txt"));
        assert!(diff.contains("+Hello, world!"));
    }

    #[test]
    fn modified_file_diff_shows_replacements() {
        let old = "fn old_one() {}\nfn old_two() {}\n";
        let new = "fn new_one() {}\nfn new_two() {}\n";
        let path = PathBuf::from("src/lib.rs");
        let diff = unified_diff(&path, Some(old), new);

        assert!(diff.contains("--- src/lib.rs"));
        assert!(diff.contains("+++ src/lib.rs"));
        assert!(diff.contains("-fn old_one() {}"));
        assert!(diff.contains("+fn new_one() {}"));
        assert!(diff.contains("-fn old_two() {}"));
        assert!(diff.contains("+fn new_two() {}"));
    }

    #[test]
    fn unchanged_content_produces_empty_diff() {
        let text = "fn main() {}\n";
        let diff = unified_diff(Path::new("main.rs"), Some(text), text);
        assert!(!diff.contains("@@"));
        assert!(!diff.contains("\n+"));
        assert!(!diff.contains("\n-"));
    }
}
