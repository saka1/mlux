use typst::diag::FileResult;
use typst::foundations::{Bytes, Datetime};
use typst::syntax::{FileId, Source, VirtualPath};
use typst::text::{Font, FontBook};
use typst::utils::LazyHash;
use typst::{Library, LibraryExt, World};
use typst_kit::fonts::{FontSearcher, FontSlot, Fonts};

/// The Typst world for mlux.
///
/// Provides a single virtual file (`/main.typ`) containing the theme
/// set-rules followed by the converted Markdown content.
pub struct MluxWorld {
    library: LazyHash<Library>,
    book: LazyHash<FontBook>,
    fonts: Vec<FontSlot>,
    main_id: FileId,
    main_source: Source,
}

impl MluxWorld {
    /// Create a new MluxWorld.
    ///
    /// - `theme_text`: contents of the theme.typ file
    /// - `content_text`: Typst markup converted from Markdown
    /// - `width`: page width in pt
    pub fn new(theme_text: &str, content_text: &str, width: f64) -> Self {
        // Inline theme + width override + content into a single source
        let main_text = format!(
            "{theme_text}\n#set page(width: {width}pt)\n{content_text}\n"
        );

        Self::from_source(&main_text, true)
    }

    /// Create a MluxWorld from raw Typst source (no theme injection or width override).
    pub fn new_raw(source: &str) -> Self {
        Self::from_source(source, false)
    }

    fn from_source(main_text: &str, check_cjk: bool) -> Self {
        let vpath = VirtualPath::new("main.typ");
        let main_id = FileId::new(None, vpath);
        let main_source = Source::new(main_id, main_text.to_string());

        let Fonts { book, fonts } = FontSearcher::new()
            .include_system_fonts(true)
            .search();

        if check_cjk {
            // Check if any CJK font is available
            // FontBook uses lowercased family names
            let has_cjk = book.contains_family("ipagothic")
                || book.contains_family("noto sans cjk jp")
                || book.contains_family("noto serif cjk jp")
                || book.contains_family("ipamincho");
            if !has_cjk {
                eprintln!("warning: no CJK font found. Japanese text may not render correctly.");
                eprintln!("  Install IPAGothic or Noto Sans CJK JP for proper rendering.");
            }
        }

        Self {
            library: LazyHash::new(Library::default()),
            book: LazyHash::new(book),
            fonts,
            main_id,
            main_source,
        }
    }
}

impl World for MluxWorld {
    fn library(&self) -> &LazyHash<Library> {
        &self.library
    }

    fn book(&self) -> &LazyHash<FontBook> {
        &self.book
    }

    fn main(&self) -> FileId {
        self.main_id
    }

    fn source(&self, id: FileId) -> FileResult<Source> {
        if id == self.main_id {
            Ok(self.main_source.clone())
        } else {
            Err(typst::diag::FileError::NotFound(
                id.vpath().as_rootless_path().into(),
            ))
        }
    }

    fn file(&self, id: FileId) -> FileResult<Bytes> {
        if id == self.main_id {
            Ok(Bytes::from_string(self.main_source.clone()))
        } else {
            Err(typst::diag::FileError::NotFound(
                id.vpath().as_rootless_path().into(),
            ))
        }
    }

    fn font(&self, index: usize) -> Option<Font> {
        self.fonts.get(index)?.get()
    }

    fn today(&self, _offset: Option<i64>) -> Option<Datetime> {
        None
    }
}
