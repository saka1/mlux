use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};

/// State tracking for nested containers during conversion.
#[derive(Debug)]
enum Container {
    Heading,
    Strong,
    Emphasis,
    Strikethrough,
    Link { _url: String },
    BlockQuote,
    List { ordered: bool },
    Item,
    CodeBlock,
    Table { _col_count: usize },
    TableHead,
    TableRow,
    TableCell,
}

/// Convert Markdown text to Typst markup.
pub fn markdown_to_typst(markdown: &str) -> String {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    let parser = Parser::new_ext(markdown, options);

    let mut output = String::new();
    let mut stack: Vec<Container> = Vec::new();
    let mut in_code_block = false;
    // Buffer for collecting table cell content
    let mut cell_buf: Option<String> = None;
    // Collected cells for current table
    let mut table_cells: Vec<String> = Vec::new();
    let mut table_col_count: usize = 0;

    for event in parser {
        match event {
            // === Block-level Start tags ===
            Event::Start(Tag::Paragraph) => {
                if !output.is_empty() && !output.ends_with('\n') {
                    output.push('\n');
                }
                if !output.is_empty() {
                    output.push('\n');
                }
            }
            Event::Start(Tag::Heading { level, .. }) => {
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
                if !output.is_empty() && !output.ends_with('\n') {
                    output.push('\n');
                }
                if !output.is_empty() {
                    output.push('\n');
                }
                output.push_str("#quote(block: true)[");
                stack.push(Container::BlockQuote);
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                if !output.is_empty() && !output.ends_with('\n') {
                    output.push('\n');
                }
                if !output.is_empty() {
                    output.push('\n');
                }
                in_code_block = true;
                let lang = match &kind {
                    CodeBlockKind::Fenced(lang) if !lang.is_empty() => lang.as_ref(),
                    _ => "",
                };
                output.push_str("```");
                output.push_str(lang);
                output.push('\n');
                stack.push(Container::CodeBlock);
            }
            Event::Start(Tag::List(start)) => {
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
                push_to_target(&mut output, &mut cell_buf, "*");
                stack.push(Container::Strong);
            }
            Event::Start(Tag::Emphasis) => {
                push_to_target(&mut output, &mut cell_buf, "_");
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
                push_to_target(&mut output, &mut cell_buf, &format!("#link(\"{url}\")["));
                stack.push(Container::Link { _url: url });
            }

            // === End tags ===
            Event::End(TagEnd::Paragraph) => {
                if !output.ends_with('\n') {
                    output.push('\n');
                }
            }
            Event::End(TagEnd::Heading(_)) => {
                if !output.ends_with('\n') {
                    output.push('\n');
                }
                pop_expect(&mut stack, "Heading");
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                // Trim trailing whitespace inside the quote
                let trimmed = output.trim_end().len();
                output.truncate(trimmed);
                output.push_str("]\n");
                pop_expect(&mut stack, "BlockQuote");
            }
            Event::End(TagEnd::CodeBlock) => {
                in_code_block = false;
                // Ensure the closing backticks are on their own line
                if !output.ends_with('\n') {
                    output.push('\n');
                }
                output.push_str("```\n");
                pop_expect(&mut stack, "CodeBlock");
            }
            Event::End(TagEnd::List(_)) => {
                pop_expect(&mut stack, "List");
            }
            Event::End(TagEnd::Item) => {
                if !output.ends_with('\n') {
                    output.push('\n');
                }
                pop_expect(&mut stack, "Item");
            }
            Event::End(TagEnd::Table) => {
                // Emit the entire table
                output.push_str(&format!("#table(columns: {table_col_count},\n"));
                for cell in &table_cells {
                    output.push_str(&format!("  [{cell}],\n"));
                }
                output.push_str(")\n");
                table_cells.clear();
                pop_expect(&mut stack, "Table");
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
                push_to_target(&mut output, &mut cell_buf, "*");
                pop_expect(&mut stack, "Strong");
            }
            Event::End(TagEnd::Emphasis) => {
                push_to_target(&mut output, &mut cell_buf, "_");
                pop_expect(&mut stack, "Emphasis");
            }
            Event::End(TagEnd::Strikethrough) => {
                push_to_target(&mut output, &mut cell_buf, "]");
                pop_expect(&mut stack, "Strikethrough");
            }
            Event::End(TagEnd::Link) => {
                push_to_target(&mut output, &mut cell_buf, "]");
                pop_expect(&mut stack, "Link");
            }

            // === Leaf events ===
            Event::Text(text) => {
                if in_code_block {
                    push_to_target(&mut output, &mut cell_buf, &text);
                } else if cell_buf.is_some() {
                    let escaped = escape_typst(&text);
                    cell_buf.as_mut().unwrap().push_str(&escaped);
                } else {
                    output.push_str(&escape_typst(&text));
                }
            }
            Event::Code(code) => {
                let s = format!("`{code}`");
                push_to_target(&mut output, &mut cell_buf, &s);
            }
            Event::SoftBreak => {
                if in_code_block {
                    push_to_target(&mut output, &mut cell_buf, "\n");
                } else {
                    push_to_target(&mut output, &mut cell_buf, "\n");
                }
            }
            Event::HardBreak => {
                push_to_target(&mut output, &mut cell_buf, "\\ \n");
            }
            Event::Rule => {
                if !output.is_empty() && !output.ends_with('\n') {
                    output.push('\n');
                }
                if !output.is_empty() {
                    output.push('\n');
                }
                output.push_str("#line(length: 100%)\n");
            }
            _ => {}
        }
    }

    if !output.is_empty() && !output.ends_with('\n') {
        output.push('\n');
    }

    output
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

/// Escape characters that have special meaning in Typst markup.
fn escape_typst(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '#' | '*' | '_' | '`' | '<' | '>' | '@' | '$' | '\\' | '/' | '~' => {
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
        assert!(typst.contains("*bold*"));
        assert!(typst.contains("_italic_"));
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
    fn test_link() {
        let md = "[Rust](https://www.rust-lang.org/)";
        let typst = markdown_to_typst(md);
        assert!(typst.contains("#link(\"https://www.rust-lang.org/\")[Rust]"));
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
}
