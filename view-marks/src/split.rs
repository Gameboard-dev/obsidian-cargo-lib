//! A note's prose paragraphs: runs of lines between blank lines, each list item on its own, leaving out the front
//! matter, code and maths blocks, multi-line comments, headings, tables, embeds, block ids, dividers and footnote
//! definitions. One pass over the text, no regular expressions.
//!
//! The syntax: Obsidian's comments, embeds and block ids (<https://help.obsidian.md/syntax>), CommonMark's list
//! items, fences and thematic breaks (<https://spec.commonmark.org/0.31.2/>). Offsets are counted in UTF-16, as
//! JavaScript strings are (<https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/String#utf-16_characters_unicode_code_points_and_grapheme_clusters>).

use xxhash_rust::xxh3::xxh3_64;

#[derive(Clone, Debug, PartialEq)]
pub struct Para {
    /// UTF-16 offsets in the note (what CodeMirror and JavaScript index by).
    pub from16: u32,
    pub to16: u32,
    /// 0-based line numbers, inclusive (what Obsidian's section info gives).
    pub line_from: u32,
    pub line_to: u32,
    /// Byte range in the note's UTF-8 text.
    pub start: usize,
    pub end: usize,
    /// xxh3 of the paragraph's text (<https://docs.rs/xxhash-rust>): the cache key, so an unchanged paragraph is never
    /// sent again.
    pub id: u64,
    pub words: u32,
}

#[derive(Clone, Copy, PartialEq)]
enum Block {
    Fence,
    Math,
    Comment,
    Front,
}

fn is_fence(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("```") || t.starts_with("~~~")
}

fn is_heading(t: &str) -> bool {
    let hashes = t.bytes().take_while(|&b| b == b'#').count();
    (1..=6).contains(&hashes) && t[hashes..].starts_with(|c: char| c.is_whitespace())
}

fn is_divider(t: &str) -> bool {
    let t = t.trim_end();
    t.len() >= 3 && (t.bytes().all(|b| b == b'-') || t.bytes().all(|b| b == b'_') || t.bytes().all(|b| b == b'*'))
}

fn is_block_id(t: &str) -> bool {
    let t = t.trim_end();
    t.len() > 1 && t.starts_with('^') && t[1..].bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn is_footnote_def(t: &str) -> bool {
    t.starts_with("[^") && t.find("]:").is_some_and(|i| i > 2 && !t[2..i].contains(']'))
}

/// Headings, tables, embeds, HTML comments, block ids, dividers, footnote definitions.
fn is_skip(line: &str) -> bool {
    let t = line.trim_start();
    is_heading(t) || t.starts_with('|') || t.starts_with("![[") || t.starts_with("<!--") || is_block_id(t) || is_divider(t)
        || is_footnote_def(t)
}

/// A list item's first line: `- `, `* `, `+ `, `1. `, `1) `.
fn is_item(line: &str) -> bool {
    let t = line.trim_start();
    let b = t.as_bytes();
    if b.len() >= 2 && matches!(b[0], b'-' | b'*' | b'+') && (b[1] as char).is_whitespace() {
        return true;
    }
    let digits = b.iter().take_while(|c| c.is_ascii_digit()).count();
    digits > 0 && b.len() > digits + 1 && matches!(b[digits], b'.' | b')') && (b[digits + 1] as char).is_whitespace()
}

pub fn split(text: &str) -> Vec<Para> {
    let mut out = Vec::new();
    let mut block: Option<Block> = None;
    // the paragraph being gathered: (first line, start byte, start utf16), and where its last line ends
    let mut cur: Option<(u32, usize, u32)> = None;
    let mut last = (0u32, 0usize, 0u32); // last line of `cur`: (line, end byte, end utf16)
    let flush = |cur: &mut Option<(u32, usize, u32)>, last: (u32, usize, u32), out: &mut Vec<Para>| {
        if let Some((line_from, start, from16)) = cur.take() {
            let s = &text[start..last.1];
            out.push(Para {
                from16,
                to16: last.2,
                line_from,
                line_to: last.0,
                start,
                end: last.1,
                id: xxh3_64(s.as_bytes()),
                words: s.split_whitespace().count() as u32,
            });
        }
    };

    let mut byte = 0usize;
    let mut u16pos = 0u32;
    for (n, line) in text.split('\n').enumerate() {
        let n = n as u32;
        let line_start = byte;
        let line_start16 = u16pos;
        let len16: u32 = if line.is_ascii() { line.len() as u32 } else { line.chars().map(|c| c.len_utf16() as u32).sum() };
        byte += line.len() + 1;
        u16pos += len16 + 1;
        let line_end = line_start + line.len();
        let line_end16 = line_start16 + len16;

        if let Some(b) = block {
            let closes = match b {
                Block::Fence => is_fence(line),
                Block::Math => line.contains("$$"),
                Block::Comment => line.contains("%%"),
                Block::Front => matches!(line.trim_end(), "---" | "..."),
            };
            if closes {
                block = None;
            }
            continue;
        }
        if n == 0 && line.trim_end() == "---" {
            block = Some(Block::Front);
            continue;
        }
        if is_fence(line) {
            flush(&mut cur, last, &mut out);
            block = Some(Block::Fence);
            continue;
        }
        let tt = line.trim();
        if tt.starts_with("$$") {
            flush(&mut cur, last, &mut out);
            if !(tt.len() >= 4 && tt.ends_with("$$")) {
                block = Some(Block::Math); // "$$ x $$" is one line
            }
            continue;
        }
        if line.trim_start().starts_with("%%") && line.matches("%%").count() == 1 {
            flush(&mut cur, last, &mut out);
            block = Some(Block::Comment);
            continue;
        }
        if tt.starts_with("%%") && tt.ends_with("%%") {
            flush(&mut cur, last, &mut out); // a comment line
            continue;
        }
        if tt.is_empty() || is_skip(line) {
            flush(&mut cur, last, &mut out);
            continue;
        }
        if is_item(line) {
            flush(&mut cur, last, &mut out);
        }
        if cur.is_none() {
            cur = Some((n, line_start, line_start16));
        }
        last = (n, line_end, line_end16);
    }
    flush(&mut cur, last, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prose_only_list_items_apart() {
        let note = [
            "---", "title: x", "---", "# Heading", "First line of a paragraph", "and its second line.", "", "- item one",
            "- item two", "```python", "code = 1", "```", "$$", "x = y", "$$", "$$ a = b $$", "| a | b |", "|---|---|",
            "%% a comment line %%", "%%", "a long", "comment", "%%", "^fig--x", "[^1]: a footnote", "___", "Last paragraph.",
        ]
        .join("\n");
        let got: Vec<&str> = split(&note).iter().map(|p| &note[p.start..p.end]).collect();
        assert_eq!(got, ["First line of a paragraph\nand its second line.", "- item one", "- item two", "Last paragraph."]);
        let p = &split(&note)[0];
        assert_eq!((p.line_from, p.line_to), (4, 5));
    }

    #[test]
    fn utf16_offsets_count_surrogate_pairs() {
        let note = "😀 first\n\nsecond é";
        let ps = split(note);
        assert_eq!((ps[0].from16, ps[0].to16), (0, 8)); // the emoji is two UTF-16 units
        assert_eq!((ps[1].from16, ps[1].to16), (10, 18));
    }

    #[test]
    fn same_text_same_id() {
        let a = split("alpha beta\n\ngamma");
        let b = split("intro\n\nalpha beta\n\ngamma delta");
        assert_eq!(a[0].id, b[1].id);
        assert_ne!(a[1].id, b[2].id);
    }
}
