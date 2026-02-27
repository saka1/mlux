use std::ops::Range;
use std::time::Instant;

use log::info;
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};

/// Markdown source line → Typst output byte range mapping for a single block.
#[derive(Debug, Clone)]
pub struct BlockMapping {
    /// Byte range within the Typst output (content_text).
    pub typst_byte_range: Range<usize>,
    /// Byte range within the original Markdown source.
    pub md_byte_range: Range<usize>,
}

/// Mapping from Typst output positions back to Markdown source positions.
#[derive(Debug, Clone)]
pub struct SourceMap {
    /// Block mappings sorted by `typst_byte_range.start` ascending.
    pub blocks: Vec<BlockMapping>,
}

impl SourceMap {
    /// Find the BlockMapping whose typst_byte_range contains `typst_offset`.
    pub fn find_by_typst_offset(&self, typst_offset: usize) -> Option<&BlockMapping> {
        // Binary search for the block whose range contains the offset.
        let idx = self
            .blocks
            .binary_search_by(|b| {
                if typst_offset < b.typst_byte_range.start {
                    std::cmp::Ordering::Greater
                } else if typst_offset >= b.typst_byte_range.end {
                    std::cmp::Ordering::Less
                } else {
                    std::cmp::Ordering::Equal
                }
            })
            .ok()?;
        Some(&self.blocks[idx])
    }
}

/// State tracking for nested containers during conversion.
#[derive(Debug)]
enum Container {
    Heading,
    Strong,
    Emphasis,
    Strikethrough,
    Link { _url: String },
    BlockQuote,
    BlockQuoteCapped,
    List { ordered: bool },
    Item,
    CodeBlock,
    Table { _col_count: usize },
    TableHead,
    TableRow,
    TableCell,
}

const MAX_BLOCKQUOTE_DEPTH: usize = 10;

/// Convert Markdown text to Typst markup (compatibility wrapper).
pub fn markdown_to_typst(markdown: &str) -> String {
    markdown_to_typst_with_map(markdown).0
}

/// Convert Markdown text to Typst markup with source mapping.
///
/// Returns the Typst markup string and a `SourceMap` that maps Typst byte
/// ranges back to the original Markdown byte ranges (block-level granularity).
pub fn markdown_to_typst_with_map(markdown: &str) -> (String, SourceMap) {
    let start = Instant::now();
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    let parser = Parser::new_ext(markdown, options);

    let mut output = String::new();
    let mut stack: Vec<Container> = Vec::new();
    let mut in_code_block = false;
    // Buffer for collecting code block content (deferred fence writing)
    let mut code_block_buf = String::new();
    let mut code_block_lang = String::new();
    // Buffer for collecting table cell content
    let mut cell_buf: Option<String> = None;
    // Collected cells for current table
    let mut table_cells: Vec<String> = Vec::new();
    let mut table_col_count: usize = 0;

    // Source mapping: track top-level block boundaries
    let mut source_map_blocks: Vec<BlockMapping> = Vec::new();
    // Stack of (typst_start, md_range) for open top-level blocks
    let mut block_starts: Vec<(usize, Range<usize>)> = Vec::new();
    // Depth counter for block-level nesting (only record at depth 0)
    let mut block_depth: usize = 0;

    for (event, md_range) in parser.into_offset_iter() {
        match event {
            // === Block-level Start tags ===
            Event::Start(Tag::Paragraph) => {
                if block_depth == 0 {
                    block_starts.push((output.len(), md_range));
                }
                block_depth += 1;
                if !output.is_empty() && !output.ends_with('\n') {
                    output.push('\n');
                }
                if !output.is_empty() {
                    output.push('\n');
                }
            }
            Event::Start(Tag::Heading { level, .. }) => {
                if block_depth == 0 {
                    block_starts.push((output.len(), md_range));
                }
                block_depth += 1;
                if !output.is_empty() && !output.ends_with('\n') {
                    output.push('\n');
                }
                if !output.is_empty() {
                    output.push('\n');
                }
                let prefix = "=".repeat(level as usize);
                output.push_str(&prefix);
                output.push(' ');
                stack.push(Container::Heading);
            }
            Event::Start(Tag::BlockQuote(_)) => {
                if block_depth == 0 {
                    block_starts.push((output.len(), md_range));
                }
                block_depth += 1;
                if !output.is_empty() && !output.ends_with('\n') {
                    output.push('\n');
                }
                if !output.is_empty() {
                    output.push('\n');
                }
                let bq_depth = stack
                    .iter()
                    .filter(|c| matches!(c, Container::BlockQuote | Container::BlockQuoteCapped))
                    .count();
                if bq_depth < MAX_BLOCKQUOTE_DEPTH {
                    output.push_str("#quote(block: true)[");
                    stack.push(Container::BlockQuote);
                } else {
                    stack.push(Container::BlockQuoteCapped);
                }
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                if block_depth == 0 {
                    block_starts.push((output.len(), md_range));
                }
                block_depth += 1;
                if !output.is_empty() && !output.ends_with('\n') {
                    output.push('\n');
                }
                if !output.is_empty() {
                    output.push('\n');
                }
                in_code_block = true;
                code_block_lang = match &kind {
                    CodeBlockKind::Fenced(lang) if !lang.is_empty() => lang.to_string(),
                    _ => String::new(),
                };
                code_block_buf.clear();
                stack.push(Container::CodeBlock);
            }
            Event::Start(Tag::List(start)) => {
                if block_depth == 0 {
                    block_starts.push((output.len(), md_range));
                }
                block_depth += 1;
                if !output.is_empty() && !output.ends_with('\n') {
                    output.push('\n');
                }
                if !output.is_empty() {
                    output.push('\n');
                }
                stack.push(Container::List {
                    ordered: start.is_some(),
                });
            }
            Event::Start(Tag::Item) => {
                if !output.is_empty() && !output.ends_with('\n') {
                    output.push('\n');
                }
                // Determine marker from parent list
                let marker = stack
                    .iter()
                    .rev()
                    .find_map(|c| match c {
                        Container::List { ordered: true } => Some("+ "),
                        Container::List { ordered: false } => Some("- "),
                        _ => None,
                    })
                    .unwrap_or("- ");
                output.push_str(marker);
                stack.push(Container::Item);
            }
            Event::Start(Tag::Table(alignments)) => {
                if block_depth == 0 {
                    block_starts.push((output.len(), md_range));
                }
                block_depth += 1;
                if !output.is_empty() && !output.ends_with('\n') {
                    output.push('\n');
                }
                if !output.is_empty() {
                    output.push('\n');
                }
                table_col_count = alignments.len();
                table_cells.clear();
                stack.push(Container::Table {
                    _col_count: table_col_count,
                });
            }
            Event::Start(Tag::TableHead) => {
                stack.push(Container::TableHead);
            }
            Event::Start(Tag::TableRow) => {
                stack.push(Container::TableRow);
            }
            Event::Start(Tag::TableCell) => {
                cell_buf = Some(String::new());
                stack.push(Container::TableCell);
            }

            // === Inline Start tags ===
            Event::Start(Tag::Strong) => {
                push_to_target(&mut output, &mut cell_buf, "#strong[");
                stack.push(Container::Strong);
            }
            Event::Start(Tag::Emphasis) => {
                push_to_target(&mut output, &mut cell_buf, "#emph[");
                stack.push(Container::Emphasis);
            }
            Event::Start(Tag::Strikethrough) => {
                push_to_target(&mut output, &mut cell_buf, "#strike[");
                stack.push(Container::Strikethrough);
            }
            Event::Start(Tag::Link {
                dest_url, ..
            }) => {
                let url = dest_url.to_string();
                if !url.is_empty() {
                    push_to_target(&mut output, &mut cell_buf, &format!("#link(\"{url}\")["));
                }
                stack.push(Container::Link { _url: url });
            }

            // === End tags ===
            Event::End(TagEnd::Paragraph) => {
                if !output.ends_with('\n') {
                    output.push('\n');
                }
                block_depth -= 1;
                if block_depth == 0
                    && let Some((typst_start, md_range_start)) = block_starts.pop()
                {
                    source_map_blocks.push(BlockMapping {
                        typst_byte_range: typst_start..output.len(),
                        md_byte_range: md_range_start,
                    });
                }
            }
            Event::End(TagEnd::Heading(_)) => {
                if !output.ends_with('\n') {
                    output.push('\n');
                }
                pop_expect(&mut stack, "Heading");
                block_depth -= 1;
                if block_depth == 0
                    && let Some((typst_start, md_range_start)) = block_starts.pop()
                {
                    source_map_blocks.push(BlockMapping {
                        typst_byte_range: typst_start..output.len(),
                        md_byte_range: md_range_start,
                    });
                }
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                match stack.pop() {
                    Some(Container::BlockQuote) => {
                        let trimmed = output.trim_end().len();
                        output.truncate(trimmed);
                        output.push_str("]\n");
                    }
                    Some(Container::BlockQuoteCapped) => {
                        // Depth-capped: no closing bracket needed
                    }
                    other => {
                        debug_assert!(false, "expected BlockQuote/BlockQuoteCapped, got {other:?}");
                    }
                }
                block_depth -= 1;
                if block_depth == 0
                    && let Some((typst_start, md_range_start)) = block_starts.pop()
                {
                    source_map_blocks.push(BlockMapping {
                        typst_byte_range: typst_start..output.len(),
                        md_byte_range: md_range_start,
                    });
                }
            }
            Event::End(TagEnd::CodeBlock) => {
                in_code_block = false;
                // Use a fence longer than any backtick run in the content
                let fence_len = max_backtick_run(&code_block_buf).max(2) + 1;
                let fence: String = "`".repeat(fence_len);
                push_to_target(&mut output, &mut cell_buf, &fence);
                push_to_target(&mut output, &mut cell_buf, &code_block_lang);
                push_to_target(&mut output, &mut cell_buf, "\n");
                push_to_target(&mut output, &mut cell_buf, &code_block_buf);
                if !code_block_buf.ends_with('\n') {
                    push_to_target(&mut output, &mut cell_buf, "\n");
                }
                push_to_target(&mut output, &mut cell_buf, &fence);
                push_to_target(&mut output, &mut cell_buf, "\n");
                code_block_buf.clear();
                code_block_lang.clear();
                pop_expect(&mut stack, "CodeBlock");
                block_depth -= 1;
                if block_depth == 0
                    && let Some((typst_start, md_range_start)) = block_starts.pop()
                {
                    source_map_blocks.push(BlockMapping {
                        typst_byte_range: typst_start..output.len(),
                        md_byte_range: md_range_start,
                    });
                }
            }
            Event::End(TagEnd::List(_)) => {
                pop_expect(&mut stack, "List");
                block_depth -= 1;
                if block_depth == 0
                    && let Some((typst_start, md_range_start)) = block_starts.pop()
                {
                    source_map_blocks.push(BlockMapping {
                        typst_byte_range: typst_start..output.len(),
                        md_byte_range: md_range_start,
                    });
                }
            }
            Event::End(TagEnd::Item) => {
                if !output.ends_with('\n') {
                    output.push('\n');
                }
                pop_expect(&mut stack, "Item");
            }
            Event::End(TagEnd::Table) => {
                // Emit the entire table
                // For tables, typst_start is recorded at Start(Table) but the
                // actual Typst output is emitted here at End(Table).
                // We need to update the typst_start to the current position
                // before emitting so the range covers the actual output.
                let table_typst_start = output.len();
                output.push_str(&format!("#table(columns: {table_col_count},\n"));
                for cell in &table_cells {
                    output.push_str(&format!("  [{cell}],\n"));
                }
                output.push_str(")\n");
                table_cells.clear();
                pop_expect(&mut stack, "Table");
                block_depth -= 1;
                if block_depth == 0
                    && let Some((_typst_start, md_range_start)) = block_starts.pop()
                {
                    source_map_blocks.push(BlockMapping {
                        typst_byte_range: table_typst_start..output.len(),
                        md_byte_range: md_range_start,
                    });
                }
            }
            Event::End(TagEnd::TableHead) => {
                pop_expect(&mut stack, "TableHead");
            }
            Event::End(TagEnd::TableRow) => {
                pop_expect(&mut stack, "TableRow");
            }
            Event::End(TagEnd::TableCell) => {
                if let Some(buf) = cell_buf.take() {
                    table_cells.push(buf);
                }
                pop_expect(&mut stack, "TableCell");
            }
            Event::End(TagEnd::Strong) => {
                push_to_target(&mut output, &mut cell_buf, "]");
                pop_expect(&mut stack, "Strong");
            }
            Event::End(TagEnd::Emphasis) => {
                push_to_target(&mut output, &mut cell_buf, "]");
                pop_expect(&mut stack, "Emphasis");
            }
            Event::End(TagEnd::Strikethrough) => {
                push_to_target(&mut output, &mut cell_buf, "]");
                pop_expect(&mut stack, "Strikethrough");
            }
            Event::End(TagEnd::Link) => {
                match stack.pop() {
                    Some(Container::Link { _url }) if !_url.is_empty() => {
                        push_to_target(&mut output, &mut cell_buf, "]");
                    }
                    Some(Container::Link { .. }) => {
                        // Empty URL — text was output as plain text, nothing to close
                    }
                    other => {
                        debug_assert!(false, "Expected Link, got {other:?}");
                    }
                }
            }

            // === Leaf events ===
            Event::Text(text) => {
                if in_code_block {
                    // Insert a space on blank lines so Typst generates a TextItem
                    // (needed for visual line extraction in the sidebar).
                    let text = fill_blank_lines(&text);
                    code_block_buf.push_str(&text);
                } else if cell_buf.is_some() {
                    let escaped = escape_typst(&text);
                    cell_buf.as_mut().unwrap().push_str(&escaped);
                } else {
                    output.push_str(&escape_typst(&text));
                }
            }
            Event::Code(code) => {
                let s = if code.contains('`') {
                    // Can't use backtick delimiters when code contains backticks;
                    // use #raw() function call instead.
                    let escaped = code.replace('\\', "\\\\").replace('"', "\\\"");
                    format!("#raw(\"{}\")", escaped)
                } else {
                    format!("`{code}`")
                };
                push_to_target(&mut output, &mut cell_buf, &s);
            }
            Event::SoftBreak => {
                if in_code_block {
                    code_block_buf.push('\n');
                } else {
                    push_to_target(&mut output, &mut cell_buf, "\n");
                }
            }
            Event::HardBreak => {
                push_to_target(&mut output, &mut cell_buf, "\\ \n");
            }
            Event::Rule => {
                let rule_start = output.len();
                if !output.is_empty() && !output.ends_with('\n') {
                    output.push('\n');
                }
                if !output.is_empty() {
                    output.push('\n');
                }
                output.push_str("#line(length: 100%)\n");
                // Only record a top-level source mapping; inside a list or other
                // block the enclosing block's range already covers this rule.
                if block_depth == 0 {
                    source_map_blocks.push(BlockMapping {
                        typst_byte_range: rule_start..output.len(),
                        md_byte_range: md_range,
                    });
                }
            }
            _ => {}
        }
    }

    if !output.is_empty() && !output.ends_with('\n') {
        output.push('\n');
    }

    let source_map = SourceMap {
        blocks: source_map_blocks,
    };
    info!(
        "convert: completed in {:.1}ms (input: {} bytes, output: {} bytes)",
        start.elapsed().as_secs_f64() * 1000.0,
        markdown.len(),
        output.len()
    );
    (output, source_map)
}

/// Push string to the cell buffer if active, otherwise to the main output.
fn push_to_target(output: &mut String, cell_buf: &mut Option<String>, s: &str) {
    if let Some(buf) = cell_buf.as_mut() {
        buf.push_str(s);
    } else {
        output.push_str(s);
    }
}

/// Pop the stack and assert the expected container type (debug only).
fn pop_expect(stack: &mut Vec<Container>, expected: &str) {
    if let Some(container) = stack.pop() {
        debug_assert!(
            matches!(
                (&container, expected),
                (Container::Heading, "Heading")
                    | (Container::Strong, "Strong")
                    | (Container::Emphasis, "Emphasis")
                    | (Container::Strikethrough, "Strikethrough")
                    | (Container::Link { .. }, "Link")
                    | (Container::BlockQuote, "BlockQuote")
                    | (Container::BlockQuoteCapped, "BlockQuote")
                    | (Container::List { .. }, "List")
                    | (Container::Item, "Item")
                    | (Container::CodeBlock, "CodeBlock")
                    | (Container::Table { .. }, "Table")
                    | (Container::TableHead, "TableHead")
                    | (Container::TableRow, "TableRow")
                    | (Container::TableCell, "TableCell")
            ),
            "Expected {expected}, got {container:?}"
        );
    }
}

/// Find the longest consecutive run of backticks in a string.
fn max_backtick_run(s: &str) -> usize {
    let mut max = 0;
    let mut current = 0;
    for ch in s.chars() {
        if ch == '`' {
            current += 1;
            if current > max {
                max = current;
            }
        } else {
            current = 0;
        }
    }
    max
}

/// Replace blank lines with space-only lines in code block text.
///
/// Typst does not generate a TextItem for empty lines in raw blocks.
/// By inserting a space, we ensure each line has a TextItem whose Y
/// coordinate can be picked up by `extract_visual_lines()`.
fn fill_blank_lines(text: &str) -> String {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut result = String::with_capacity(text.len() + lines.len());
    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            result.push('\n');
        }
        if line.is_empty() && i < lines.len() - 1 {
            result.push(' ');
        } else {
            result.push_str(line);
        }
    }
    result
}

/// Escape characters that have special meaning in Typst markup.
fn escape_typst(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '#' | '*' | '_' | '`' | '<' | '>' | '@' | '$' | '\\' | '/' | '~' | '(' | ')' | '[' | ']' => {
                escaped.push('\\');
                escaped.push(ch);
            }
            _ => escaped.push(ch),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plain_text() {
        let md = "Hello, world!";
        let typst = markdown_to_typst(md);
        assert_eq!(typst, "Hello, world!\n");
    }

    #[test]
    fn test_escape_special_chars() {
        assert_eq!(escape_typst("#hello"), "\\#hello");
        assert_eq!(escape_typst("a * b"), "a \\* b");
        assert_eq!(escape_typst("$100"), "\\$100");
        assert_eq!(escape_typst("foo(bar)"), "foo\\(bar\\)");
        assert_eq!(escape_typst("foo[bar]"), "foo\\[bar\\]");
    }

    #[test]
    fn test_emph_followed_by_paren() {
        // crash-e263b8df: **Note*(: → \*#emph[Note](: → Typst が関数呼び出しと誤解釈
        let md = "**Note*(: text";
        let typst = markdown_to_typst(md);
        assert!(
            !typst.contains("]("),
            "'](' は Typst が関数引数と解釈するため不可: {typst}"
        );
        assert!(
            typst.contains("]\\("),
            "']' の直後の '(' は '\\(' にエスケープされるべき: {typst}"
        );
    }

    #[test]
    fn test_bracket_in_heading() {
        // crash-b305e5d4: ## text](url) → ] が Typst の unexpected closing bracket
        let md = "## エanguage](https://doc.rust-lang.org/book/) を参照。";
        let typst = markdown_to_typst(md);
        assert!(
            !typst.contains("]("),
            "'](' は Typst がコンテントブロック閉じと解釈するため不可: {typst}"
        );
        assert!(
            typst.contains("\\]"),
            "テキスト中の ']' は '\\]' にエスケープされるべき: {typst}"
        );
    }

    #[test]
    fn test_brackets_in_text() {
        let md = "配列 arr[0] と [注釈] を含む文";
        let typst = markdown_to_typst(md);
        assert!(
            typst.contains("arr\\[0\\]"),
            "テキスト中の角括弧はエスケープされるべき: {typst}"
        );
        assert!(
            typst.contains("\\[注釈\\]"),
            "テキスト中の角括弧はエスケープされるべき: {typst}"
        );
    }

    #[test]
    fn test_link_text_with_bracket() {
        let md = "[foo]bar](https://example.com)";
        let typst = markdown_to_typst(md);
        // 構造的な ] は convert.rs が直接 push、テキスト中の ] は escape される
        assert!(
            !typst.contains("bar]("),
            "リンクテキスト内の ']' は '\\]' にエスケープされるべき: {typst}"
        );
    }

    #[test]
    fn test_parens_in_text() {
        let md = "関数 foo(x, y) を呼ぶ";
        let typst = markdown_to_typst(md);
        assert!(
            typst.contains("foo\\(x, y\\)"),
            "テキスト中の括弧はエスケープされるべき: {typst}"
        );
    }

    #[test]
    fn test_strong_followed_by_paren() {
        let md = "**bold** (note)";
        let typst = markdown_to_typst(md);
        assert!(
            !typst.contains("]("),
            "'](' は Typst が関数引数と解釈するため不可: {typst}"
        );
    }

    #[test]
    fn test_soft_break() {
        let md = "line1\nline2";
        let typst = markdown_to_typst(md);
        assert_eq!(typst, "line1\nline2\n");
    }

    #[test]
    fn test_hard_break() {
        let md = "line1  \nline2";
        let typst = markdown_to_typst(md);
        assert_eq!(typst, "line1\\ \nline2\n");
    }

    #[test]
    fn test_japanese_text() {
        let md = "日本語のテスト。句読点「、」も正しく処理される。";
        let typst = markdown_to_typst(md);
        assert!(typst.contains("日本語のテスト。"));
    }

    #[test]
    fn test_multiple_paragraphs() {
        let md = "段落1。\n\n段落2。";
        let typst = markdown_to_typst(md);
        assert_eq!(typst, "段落1。\n\n段落2。\n");
    }

    #[test]
    fn test_heading() {
        let md = "# Title\n\n## Subtitle";
        let typst = markdown_to_typst(md);
        assert!(typst.contains("= Title\n"));
        assert!(typst.contains("== Subtitle\n"));
    }

    #[test]
    fn test_bold_italic() {
        let md = "**bold** and *italic*";
        let typst = markdown_to_typst(md);
        assert!(typst.contains("#strong[bold]"));
        assert!(typst.contains("#emph[italic]"));
    }

    #[test]
    fn test_emphasis_function_syntax() {
        // fuzzer crash-301940: **Note*ks*: with delimiter syntax produced
        // \*\*Note_ks_: causing "unclosed delimiter" in Typst.
        // Function syntax (#emph[...]) avoids this class of bugs.
        let md = "**Note*ks*: hello";
        let typst = markdown_to_typst(md);
        assert!(
            typst.contains("#emph[ks]"),
            "should use #emph[] function syntax, got: {typst}"
        );
        assert!(
            !typst.contains("_ks_"),
            "should not produce _..._ delimiters, got: {typst}"
        );
    }

    #[test]
    fn test_strikethrough() {
        let md = "~~deleted~~";
        let typst = markdown_to_typst(md);
        assert!(typst.contains("#strike[deleted]"));
    }

    #[test]
    fn test_inline_code() {
        let md = "Use `Result<T, E>` type";
        let typst = markdown_to_typst(md);
        assert!(typst.contains("`Result<T, E>`"));
    }

    #[test]
    fn test_inline_code_with_backticks() {
        // pulldown-cmark parses `` ` `` as Code("`")
        let md = "Use `` ` `` in code";
        let typst = markdown_to_typst(md);
        assert!(
            typst.contains("#raw(\"`\")"),
            "expected #raw() call for backtick-containing code, got: {typst}"
        );
    }

    #[test]
    fn test_inline_code_with_triple_backticks() {
        // pulldown-cmark parses `` ` ``` ` `` as Code("```")
        let md = "` ``` `";
        let typst = markdown_to_typst(md);
        assert!(
            typst.contains("#raw(\"```\")"),
            "expected #raw() for triple backticks, got: {typst}"
        );
        assert!(
            !typst.contains("`````"),
            "should not produce raw backtick delimiters, got: {typst}"
        );
    }

    #[test]
    fn test_inline_code_with_backticks_in_table() {
        let md = "| Header |\n|--------|\n| `` ` `` |";
        let typst = markdown_to_typst(md);
        assert!(typst.contains("#table("), "expected table markup, got: {typst}");
        assert!(
            typst.contains("#raw(\"`\")"),
            "expected #raw() in table cell, got: {typst}"
        );
    }

    #[test]
    fn test_link() {
        let md = "[Rust](https://www.rust-lang.org/)";
        let typst = markdown_to_typst(md);
        assert!(typst.contains("#link(\"https://www.rust-lang.org/\")[Rust]"));
    }

    #[test]
    fn test_link_empty_url() {
        let md = "[link]()";
        let typst = markdown_to_typst(md);
        assert!(!typst.contains("#link"), "empty URL should not produce #link");
        assert!(typst.contains("link"));
    }

    #[test]
    fn test_code_block() {
        let md = "```rust\nfn main() {}\n```";
        let typst = markdown_to_typst(md);
        assert!(typst.contains("```rust\nfn main() {}\n```"));
    }

    #[test]
    fn test_unordered_list() {
        let md = "- item1\n- item2";
        let typst = markdown_to_typst(md);
        assert!(typst.contains("- item1\n"));
        assert!(typst.contains("- item2\n"));
    }

    #[test]
    fn test_ordered_list() {
        let md = "1. first\n2. second";
        let typst = markdown_to_typst(md);
        assert!(typst.contains("+ first\n"));
        assert!(typst.contains("+ second\n"));
    }

    #[test]
    fn test_blockquote() {
        let md = "> quoted text";
        let typst = markdown_to_typst(md);
        assert!(typst.contains("#quote(block: true)["));
        assert!(typst.contains("quoted text"));
    }

    #[test]
    fn test_horizontal_rule() {
        let md = "before\n\n---\n\nafter";
        let typst = markdown_to_typst(md);
        assert!(typst.contains("#line(length: 100%)"));
    }

    #[test]
    fn test_rule_inside_list() {
        // pulldown-cmark parses "+\t---" as an unordered list item containing
        // a thematic break (Rule event), not plain text.
        let md = "+\t---\t\t";
        let typst = markdown_to_typst(md);
        assert!(typst.contains("- "), "should produce unordered list marker");
        assert!(typst.contains("#line(length: 100%)"), "should produce horizontal rule");
    }

    #[test]
    fn test_rule_inside_list_source_map() {
        // crash-823d13a0: list item containing --- emitted a Rule source mapping
        // that overlapped with the enclosing List block mapping.
        let md = "+\t---\t\t";
        let (typst, map) = markdown_to_typst_with_map(md);
        for pair in map.blocks.windows(2) {
            assert!(
                pair[0].typst_byte_range.end <= pair[1].typst_byte_range.start,
                "overlapping typst ranges: {:?} and {:?}",
                pair[0].typst_byte_range,
                pair[1].typst_byte_range,
            );
        }
        for block in &map.blocks {
            assert!(
                block.typst_byte_range.end <= typst.len(),
                "typst_byte_range {:?} out of bounds",
                block.typst_byte_range,
            );
        }
    }

    #[test]
    fn test_table() {
        let md = "| A | B |\n|---|---|\n| 1 | 2 |";
        let typst = markdown_to_typst(md);
        assert!(typst.contains("#table(columns: 2,"));
        assert!(typst.contains("[A]"));
        assert!(typst.contains("[B]"));
        assert!(typst.contains("[1]"));
        assert!(typst.contains("[2]"));
    }

    #[test]
    fn test_code_block_no_escape() {
        // Characters inside code blocks should NOT be escaped
        let md = "```\n#hello *world* $100\n```";
        let typst = markdown_to_typst(md);
        assert!(
            typst.contains("#hello *world* $100"),
            "Code block content should not be escaped, got: {typst}"
        );
    }

    #[test]
    fn test_code_block_blank_lines_filled() {
        let md = "```\nline1\n\nline3\n```";
        let typst = markdown_to_typst(md);
        // Blank line should be replaced with a space
        assert!(
            typst.contains("line1\n \nline3"),
            "Blank lines in code blocks should be filled with a space, got: {typst}"
        );
    }

    #[test]
    fn test_code_block_multiple_blank_lines() {
        let md = "```\nline1\n\n\nline4\n```";
        let typst = markdown_to_typst(md);
        // Two consecutive blank lines should each get a space
        assert!(
            typst.contains("line1\n \n \nline4"),
            "Multiple blank lines should each be filled, got: {typst}"
        );
    }

    #[test]
    fn test_code_block_containing_backtick_fence() {
        let md = "````\n```rust\nfn main() {}\n```\n````";
        let typst = markdown_to_typst(md);
        // The generated fence must be longer than the 3-backtick run inside
        assert!(
            typst.contains("````"),
            "fence should be at least 4 backticks, got: {typst}"
        );
        assert!(
            typst.contains("```rust\nfn main() {}\n```"),
            "content should be preserved verbatim, got: {typst}"
        );
    }

    #[test]
    fn test_max_backtick_run() {
        assert_eq!(max_backtick_run(""), 0);
        assert_eq!(max_backtick_run("no backticks"), 0);
        assert_eq!(max_backtick_run("a`b"), 1);
        assert_eq!(max_backtick_run("```"), 3);
        assert_eq!(max_backtick_run("a```b``c"), 3);
        assert_eq!(max_backtick_run("``````"), 6);
    }

    #[test]
    fn test_blockquote_depth_capped() {
        // 15段ネスト → 最初の10段のみ #quote 出力
        let input = "> ".repeat(15) + "deep";
        let result = markdown_to_typst(&input);
        let quote_count = result.matches("#quote(block: true)[").count();
        assert_eq!(quote_count, MAX_BLOCKQUOTE_DEPTH);
        assert!(result.contains("deep"));
    }

    #[test]
    fn test_fill_blank_lines() {
        assert_eq!(fill_blank_lines("a\n\nb\n"), "a\n \nb\n");
        assert_eq!(fill_blank_lines("a\n\n\nb\n"), "a\n \n \nb\n");
        assert_eq!(fill_blank_lines("a\nb\n"), "a\nb\n"); // no blanks
        assert_eq!(fill_blank_lines("a\n"), "a\n"); // trailing newline preserved
    }
}
