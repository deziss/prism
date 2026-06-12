// PRISM utils.rs — shared utilities

use std::path::Path;
use std::fs;

/// Get file extension as a string
pub fn file_ext(path: &Path) -> Option<&str> {
    path.extension()?.to_str()
}

/// Recursively walk a directory and return sorted list of files
pub fn list_files(root: &Path, extensions: &[&str]) -> Result<Vec<String>, std::io::Error> {
    let mut files = Vec::new();
    for entry in walkdir::WalkDir::new(root) {
        let entry = entry?;
        if entry.file_type().is_file() {
            if let Some(ext) = entry.path().extension().and_then(|e| e.to_str()) {
                if extensions.contains(&ext) {
                    files.push(entry.path().display().to_string());
                }
            }
        }
    }
    files.sort();
    Ok(files)
}

/// Sanitize a filename (remove special chars)
pub fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '_' })
        .collect()
}

/// Check if a value is a valid number
pub fn is_number(s: &str) -> bool {
    s.parse::<f64>().is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_filename() {
        assert_eq!(sanitize_filename("hello/world.txt"), "hello_world.txt");
        assert_eq!(sanitize_filename("my-file_v2.rs"), "my-file_v2.rs");
    }

    #[test]
    fn test_is_number() {
        assert!(is_number("123"));
        assert!(is_number("-45.67"));
        assert!(!is_number("abc"));
    }
}