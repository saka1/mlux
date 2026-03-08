use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::Path;

use log::info;
use typst::foundations::Bytes;

const MAX_IMAGE_SIZE: u64 = 50 * 1024 * 1024; // 50 MB

/// A validated set of loaded image assets (path → bytes).
#[derive(Clone, Default, Debug)]
pub struct LoadedImages {
    inner: HashMap<String, Bytes>,
}

impl LoadedImages {
    pub fn get(&self, path: &str) -> Option<&Bytes> {
        self.inner.get(path)
    }

    pub fn key_set(&self) -> HashSet<String> {
        self.inner.keys().cloned().collect()
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    pub fn insert(&mut self, key: String, data: Bytes) {
        self.inner.insert(key, data);
    }
}

#[derive(Debug)]
pub enum ImageError {
    AbsolutePath(String),
    RemoteUrl(String),
    OutsideBase(String),
    TooLarge(String, u64),
    IoError(String, std::io::Error),
}

impl fmt::Display for ImageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ImageError::AbsolutePath(p) => write!(f, "image: absolute path rejected: {p}"),
            ImageError::RemoteUrl(p) => write!(f, "image: remote URL rejected: {p}"),
            ImageError::OutsideBase(p) => write!(f, "image: path outside base directory: {p}"),
            ImageError::TooLarge(p, size) => {
                write!(f, "image: file too large ({size} bytes): {p}")
            }
            ImageError::IoError(p, e) => write!(f, "image: failed to read {p}: {e}"),
        }
    }
}

/// Load image files from disk, validating paths for security.
///
/// Returns a map of path → bytes for successfully loaded images,
/// and a list of errors for images that could not be loaded.
///
/// If `base_dir` is `None` (stdin mode), returns an empty map.
pub fn load_images(
    image_paths: &[String],
    base_dir: Option<&Path>,
) -> (LoadedImages, Vec<ImageError>) {
    let mut loaded = HashMap::new();
    let mut errors = Vec::new();

    let Some(base_dir) = base_dir else {
        return (LoadedImages::default(), errors);
    };

    // Deduplicate paths
    let mut seen = HashSet::new();

    for path_str in image_paths {
        if !seen.insert(path_str) {
            continue;
        }

        // Reject remote URLs
        if path_str.starts_with("http://")
            || path_str.starts_with("https://")
            || path_str.starts_with("data:")
        {
            errors.push(ImageError::RemoteUrl(path_str.clone()));
            continue;
        }

        let path = Path::new(path_str);

        // Reject absolute paths
        if path.is_absolute() {
            errors.push(ImageError::AbsolutePath(path_str.clone()));
            continue;
        }

        // Resolve and validate path is under base_dir
        let full_path = base_dir.join(path);
        let canonical = match full_path.canonicalize() {
            Ok(p) => p,
            Err(e) => {
                errors.push(ImageError::IoError(path_str.clone(), e));
                continue;
            }
        };
        let canonical_base = match base_dir.canonicalize() {
            Ok(p) => p,
            Err(e) => {
                errors.push(ImageError::IoError(path_str.clone(), e));
                continue;
            }
        };
        if !canonical.starts_with(&canonical_base) {
            errors.push(ImageError::OutsideBase(path_str.clone()));
            continue;
        }

        // Check file size
        let metadata = match std::fs::metadata(&canonical) {
            Ok(m) => m,
            Err(e) => {
                errors.push(ImageError::IoError(path_str.clone(), e));
                continue;
            }
        };
        if metadata.len() > MAX_IMAGE_SIZE {
            errors.push(ImageError::TooLarge(path_str.clone(), metadata.len()));
            continue;
        }

        // Read the file
        match std::fs::read(&canonical) {
            Ok(data) => {
                loaded.insert(path_str.clone(), Bytes::new(data));
            }
            Err(e) => {
                errors.push(ImageError::IoError(path_str.clone(), e));
            }
        }
    }

    info!(
        "image: loaded {} images, {} errors",
        loaded.len(),
        errors.len()
    );
    (LoadedImages { inner: loaded }, errors)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_absolute_path_rejected() {
        let paths = vec!["/etc/passwd".to_string()];
        let (loaded, errors) = load_images(&paths, Some(Path::new(".")));
        assert!(loaded.is_empty());
        assert_eq!(errors.len(), 1);
        assert!(matches!(&errors[0], ImageError::AbsolutePath(_)));
    }

    #[test]
    fn test_url_rejected() {
        let paths = vec![
            "https://example.com/img.png".to_string(),
            "http://example.com/img.png".to_string(),
            "data:image/png;base64,abc".to_string(),
        ];
        let (loaded, errors) = load_images(&paths, Some(Path::new(".")));
        assert!(loaded.is_empty());
        assert_eq!(errors.len(), 3);
        for err in &errors {
            assert!(matches!(err, ImageError::RemoteUrl(_)));
        }
    }

    #[test]
    fn test_path_traversal_rejected() {
        let paths = vec!["../../../etc/passwd".to_string()];
        let (loaded, errors) = load_images(&paths, Some(Path::new("/tmp")));
        assert!(loaded.is_empty());
        assert!(!errors.is_empty());
    }

    #[test]
    fn test_none_base_dir() {
        let paths = vec!["image.png".to_string()];
        let (loaded, errors) = load_images(&paths, None);
        assert!(loaded.is_empty());
        assert!(errors.is_empty());
    }

    #[test]
    fn test_load_valid_image() {
        let dir = std::env::temp_dir().join("mlux_test_image");
        fs::create_dir_all(&dir).unwrap();
        let img_path = dir.join("test.png");
        // Minimal PNG
        let png_data = [
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, // signature
            0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, // IHDR
            0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90,
            0x77, 0x53, 0xDE, // CRC
            0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, // IDAT
            0x08, 0xD7, 0x63, 0xF8, 0xCF, 0xC0, 0x00, 0x00, 0x00, 0x02, 0x00, 0x01, 0xE2, 0x21,
            0xBC, 0x33, // CRC
            0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, // IEND
            0xAE, 0x42, 0x60, 0x82,
        ];
        fs::write(&img_path, png_data).unwrap();

        let paths = vec!["test.png".to_string()];
        let (loaded, errors) = load_images(&paths, Some(&dir));
        assert!(errors.is_empty(), "errors: {errors:?}");
        assert_eq!(loaded.len(), 1);
        assert!(loaded.get("test.png").is_some());

        // Cleanup
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_deduplication() {
        let paths = vec!["nonexistent.png".to_string(), "nonexistent.png".to_string()];
        let (_, errors) = load_images(&paths, Some(Path::new(".")));
        // Should only produce one error (deduplicated)
        assert_eq!(errors.len(), 1);
    }
}
