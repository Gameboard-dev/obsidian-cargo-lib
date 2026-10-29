//! Copy with Augmented View highlights (docs/colour-codes.md, "Copying with highlights"): a selection's Markdown with
//! each coloured passage wrapped in a marker, `~={colour, category}text=~` (the default) or
//! `<mark data-colour="…" data-category="…">text</mark>`, for the clipboard. [`write_range`] is a pure function of the
//! note's text, the selection and the marks the worker computed for it (the same ones the editor draws): the note is
//! never changed, and nothing is written but the clipboard.
//!
//! Markers never break the Markdown around them:
//! - nothing is marked in front matter, code blocks, maths blocks, comment blocks, dividers, embeds or block-id lines;
//! - a marker covers only a line's text, never its indent, blockquote `>`, callout header, list marker, task box,
//!   heading `#`, footnote label, a trailing block id (` ^id`) or the line break: a passage over several lines gets a
//!   marker on each line;
//! - a marker never starts or ends inside inline code, inline maths, a comment, a link or embed (its target and its
//!   text), a footnote, a citation, an HTML tag or a bare URL: it grows to take the whole of it, or, where the
//!   selection itself cuts through one, leaves the cut piece out;
//! - a selection inside a code block, maths or a comment is copied as it is, without markers;
//! - an unescaped `|` (a table's cells) stays outside: the marker is split around it;
//! - markers nest (a phrase inside its sentence); where two would cross, the later one is split at the earlier one's end;
//! - with the `~=` markers, where a marker would touch a `~` or `=` and make `~~` (strikethrough) or `==` (a highlight)
//!   that was not in the note, an empty HTML comment (`<!---->`, which Obsidian does not show) is put between them.
//!
//! The syntax followed: Obsidian's (<https://help.obsidian.md/syntax>) and CommonMark's
//! (<https://spec.commonmark.org/0.31.2/>: code spans, links, raw HTML, backslash escapes, entity references).

use std::cmp::Reverse;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Format {
    /// `~={colour, category}text=~`
    Tilde,
    /// `<mark data-colour="colour" data-category="category">text</mark>`
    Html,
}

impl Format {
    /// The setting's value ("tilde" or "html"); anything else is the default, `~=`.
    pub fn from_setting(s: &str) -> Format {
        if s.trim().eq_ignore_ascii_case("html") { Format::Html } else { Format::Tilde }
    }
}

/// A coloured passage: [from, to) in the note's UTF-16 offsets (the worker's marks), the colour's index in the list
/// given to [`write`], and whether it is a phrase colour (drawn inside a sentence's).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mark {
    pub from: u32,
    pub to: u32,
    pub colour: usize,
    pub phrase: bool,
}

/// A colour as a marker names it: its colour as the code defines it (the hex) and its name (the category).
#[derive(Clone, Debug, PartialEq)]
pub struct Named {
    pub colour: String,
    pub name: String,
}

/// Between a marker and a `~` or `=` it would otherwise touch (see the module's notes).
const SEPARATOR: &str = "<!---->";

/// The whole text with its marks as markers (the tests' shorthand for [`write_range`] over all of it).
#[cfg(test)]
pub fn write(text: &str, marks: &[Mark], colours: &[Named], format: Format) -> (String, usize) {
    write_range(text, marks, colours, format, 0, u32::MAX)
}

/// The text [from, to) (UTF-16 offsets: a selection) with its marks as markers, and how many markers were written. The
/// note's whole text is given, so that what surrounds the selection (a code block opened above it, the line's list
/// marker) is known. A mark is cut to the selection; where the selection cuts through a link, code or another atom,
/// the marker leaves the cut piece out rather than take in what was not selected. Marks of no colour in `colours`, or
/// only over structure (a list marker, a heading's `#`), write nothing.
pub fn write_range(text: &str, marks: &[Mark], colours: &[Named], format: Format, from: u32, to: u32) -> (String, usize) {
    let lines = lines(text);
    let conv = Utf16::new(text);
    let (sa, se) = (conv.byte(from), conv.byte(to.max(from)));
    let mut pieces: Vec<Piece> = Vec::new();
    for m in marks.iter().filter(|m| m.colour < colours.len() && m.from < m.to) {
        let (from, to) = (conv.byte(m.from).max(sa), conv.byte(m.to).min(se));
        if from >= to {
            continue;
        }
        let first = lines.partition_point(|l| l.end <= from);
        for l in &lines[first..] {
            if l.start >= to {
                break;
            }
            let Some(c) = &l.content else { continue };
            let (mut a, mut e) = (from.max(c.start), to.min(c.end));
            if a >= e {
                continue;
            }
            for &(s, t) in &c.atoms {
                // a boundary inside an atom takes all of it, or, where the selection cuts the atom, none of it
                if s < a && a < t {
                    a = if s >= sa { s } else { t };
                }
                if s < e && e < t {
                    e = if t <= se { t } else { s };
                }
            }
            if a >= e {
                continue;
            }
            let mut start = a;
            for &p in c.pipes.iter().filter(|&&p| p >= a && p < e) {
                pieces.extend(Piece::trimmed(text, start, p, m));
                start = p + 1;
            }
            pieces.extend(Piece::trimmed(text, start, e, m));
        }
    }
    let pieces = nest(text, merge(pieces));
    let atoms: Vec<(usize, usize)> = lines.iter().filter_map(|l| l.content.as_ref()).flat_map(|c| c.atoms.iter().copied()).collect();
    (emit(text, &pieces, colours, format, &atoms, (sa, se)), pieces.len())
}

// ----- lines and what in them may be marked

/// A line ([start, end) in bytes, without its line break) and the part of it a marker may cover: None for a line
/// that is not prose (front matter, a code or maths block, a comment, a divider, an embed, a block id).
struct Line {
    start: usize,
    end: usize,
    content: Option<Content>,
}

struct Content {
    start: usize,
    end: usize,
    /// Inline code, maths, comments, links, embeds, footnotes, citations, HTML tags, bare URLs: [start, end) in bytes.
    atoms: Vec<(usize, usize)>,
    /// Unescaped `|` outside them.
    pipes: Vec<usize>,
}

#[derive(Clone, Copy, PartialEq)]
enum Block {
    Fence,
    Math,
    Comment,
    Html,
}

fn is_fence(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("```") || t.starts_with("~~~")
}

fn is_divider(t: &str) -> bool {
    let s: String = t.chars().filter(|c| !c.is_whitespace()).collect();
    s.len() >= 3 && (s.bytes().all(|b| b == b'-') || s.bytes().all(|b| b == b'_') || s.bytes().all(|b| b == b'*'))
}

/// A table's delimiter row (`|---|:--:|`).
fn is_table_rule(t: &str) -> bool {
    t.contains('|') && t.contains('-') && t.bytes().all(|b| matches!(b, b'|' | b'-' | b':' | b' ' | b'\t'))
}

fn is_block_id(t: &str) -> bool {
    t.len() > 1 && t.starts_with('^') && t[1..].bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// The byte just after the front matter's closing line (its line break included); 0 when there is none.
fn front_matter_end(text: &str) -> usize {
    let mut at = 0;
    for (n, line) in text.split('\n').enumerate() {
        let t = line.trim_end();
        at += line.len() + 1;
        if n == 0 && t != "---" {
            return 0;
        }
        if n > 0 && (t == "---" || t == "...") {
            return at.min(text.len());
        }
    }
    0 // never closed: not front matter
}

fn lines(text: &str) -> Vec<Line> {
    let mut out = Vec::new();
    let mut block: Option<Block> = None;
    let mut at = 0;
    let fm_end = front_matter_end(text);
    for line in text.split('\n') {
        let (start, end) = (at, at + line.len());
        at = end + 1;
        let t = line.trim();
        let opaque = if start < fm_end {
            true
        } else if let Some(b) = block {
            let closes = match b {
                Block::Fence => is_fence(line),
                Block::Math => line.contains("$$"),
                Block::Comment => line.contains("%%"),
                Block::Html => line.contains("-->"),
            };
            if closes {
                block = None;
            }
            true
        } else if is_fence(line) {
            block = Some(Block::Fence);
            true
        } else if t.starts_with("$$") {
            if !(t.len() >= 4 && t.ends_with("$$")) {
                block = Some(Block::Math);
            }
            true
        } else if t.starts_with("%%") && line.matches("%%").count() == 1 {
            block = Some(Block::Comment);
            true
        } else if t.starts_with("<!--") && !t.contains("-->") {
            block = Some(Block::Html);
            true
        } else {
            (t.starts_with("%%") && t.ends_with("%%")) || t.is_empty() || is_divider(t) || is_block_id(t) || t.starts_with("![[") || is_table_rule(t)
        };
        let content = (!opaque).then(|| {
            let cs = prefix_len(line);
            let ce = content_end(line, cs);
            let (atoms, pipes) = scan(&line[cs..ce], start + cs);
            Content { start: start + cs, end: start + ce, atoms, pipes }
        });
        out.push(Line { start, end, content });
    }
    out
}

/// The line's structure before its text: indent, blockquote `>`s, a callout's `[!type]`, a heading's `#`s, a list
/// marker and its task box, a footnote definition's label.
fn prefix_len(line: &str) -> usize {
    let b = line.as_bytes();
    let n = b.len();
    let ws = |i: &mut usize| {
        while *i < n && (b[*i] == b' ' || b[*i] == b'\t') {
            *i += 1;
        }
    };
    let mut i = 0;
    let mut quoted = false;
    loop {
        ws(&mut i);
        if i < n && b[i] == b'>' {
            i += 1;
            quoted = true;
        } else {
            break;
        }
    }
    if quoted && line[i..].starts_with("[!") {
        if let Some(j) = line[i..].find(']') {
            i += j + 1;
            if i < n && (b[i] == b'+' || b[i] == b'-') {
                i += 1;
            }
        }
    }
    let hashes = b[i..].iter().take_while(|&&c| c == b'#').count();
    let space = |j: usize| j >= n || b[j] == b' ' || b[j] == b'\t';
    if (1..=6).contains(&hashes) && space(i + hashes) {
        i += hashes;
    } else if i < n && matches!(b[i], b'-' | b'*' | b'+') && i + 1 < n && space(i + 1) {
        i += 1;
        ws(&mut i);
        if i + 2 < n && b[i] == b'[' && b[i + 2] == b']' && space(i + 3) {
            i += 3; // a task's box
        }
    } else {
        let digits = b[i..].iter().take_while(|c| c.is_ascii_digit()).count();
        if digits > 0 && i + digits + 1 < n && matches!(b[i + digits], b'.' | b')') && space(i + digits + 1) {
            i += digits + 1;
            ws(&mut i);
            if i + 2 < n && b[i] == b'[' && b[i + 2] == b']' && space(i + 3) {
                i += 3;
            }
        } else if line[i..].starts_with("[^") {
            if let Some(j) = line[i..].find("]:") {
                i += j + 2; // a footnote definition's label
            }
        }
    }
    ws(&mut i);
    i
}

/// Where the line's text ends: before trailing spaces and a trailing block id (` ^id`), never before `from`.
fn content_end(line: &str, from: usize) -> usize {
    let mut e = line.trim_end().len();
    if let Some(sp) = line[..e].rfind([' ', '\t']) {
        if sp >= from && is_block_id(&line[sp + 1..e]) {
            e = line[..sp].trim_end().len();
        }
    }
    e.max(from)
}

/// The atoms (see [`Content`]) and unescaped pipes of a line's text `s`, which starts at byte `base` of the note.
fn scan(s: &str, base: usize) -> (Vec<(usize, usize)>, Vec<usize>) {
    let b = s.as_bytes();
    let n = b.len();
    let (mut atoms, mut pipes) = (Vec::new(), Vec::new());
    let next_char = |i: usize| s[i..].chars().next().map_or(1, char::len_utf8);
    let mut i = 0;
    while i < n {
        let rest = &s[i..];
        let atom = match b[i] {
            b'\\' => {
                i += 1 + if i + 1 < n { next_char(i + 1) } else { 0 };
                continue;
            }
            b'`' => {
                let run = b[i..].iter().take_while(|&&c| c == b'`').count();
                match closing_ticks(s, i + run, run) {
                    Some(j) => Some(j + run),
                    None => {
                        i += run;
                        continue;
                    }
                }
            }
            b'$' if rest.starts_with("$$") => s[i + 2..].find("$$").map(|j| i + 2 + j + 2),
            b'$' => unescaped(s, i + 1, b'$').filter(|&j| j > i + 1 && !b[i + 1].is_ascii_whitespace() && !b[j - 1].is_ascii_whitespace()).map(|j| j + 1),
            b'%' if rest.starts_with("%%") => s[i + 2..].find("%%").map(|j| i + 2 + j + 2),
            b'<' if rest.starts_with("<!--") => s[i + 4..].find("-->").map(|j| i + 4 + j + 3),
            b'<' if i + 1 < n && (b[i + 1].is_ascii_alphabetic() || b[i + 1] == b'/' || b[i + 1] == b'!') => s[i..].find('>').map(|j| i + j + 1),
            b'!' if rest.starts_with("![") => bracket_end(s, i + 1, true),
            b'^' if rest.starts_with("^[") => bracket_end(s, i + 1, true),
            b'[' => bracket_end(s, i, false),
            b'h' if (rest.starts_with("http://") || rest.starts_with("https://")) && (i == 0 || !b[i - 1].is_ascii_alphanumeric()) => {
                let end = rest.find(|c: char| c.is_whitespace() || c == '<').map_or(n, |j| i + j);
                Some(i + s[i..end].trim_end_matches(['.', ',', ';', ':', '!', '?', ')', '"', '\'']).len())
            }
            b'|' => {
                pipes.push(base + i);
                None
            }
            _ => None,
        };
        match atom {
            Some(e) if e > i => {
                atoms.push((base + i, base + e));
                i = e;
            }
            _ => i += next_char(i),
        }
    }
    (atoms, pipes)
}

/// The start of the next run of exactly `run` backticks from `from` (a code span's end).
fn closing_ticks(s: &str, from: usize, run: usize) -> Option<usize> {
    let b = s.as_bytes();
    let mut i = from;
    while i < b.len() {
        if b[i] == b'`' {
            let r = b[i..].iter().take_while(|&&c| c == b'`').count();
            if r == run {
                return Some(i);
            }
            i += r;
        } else {
            i += 1;
        }
    }
    None
}

/// The next `c` from `from` that no backslash escapes.
fn unescaped(s: &str, from: usize, c: u8) -> Option<usize> {
    let b = s.as_bytes();
    let mut i = from;
    while i < b.len() {
        match b[i] {
            b'\\' => i += 2,
            x if x == c => return Some(i),
            _ => i += 1,
        }
    }
    None
}

/// The bracket matching the one at `i` (`open`), nesting and escapes followed.
fn matching(s: &str, i: usize, open: u8, close: u8) -> Option<usize> {
    let b = s.as_bytes();
    let (mut depth, mut j) = (0usize, i);
    while j < b.len() {
        match b[j] {
            b'\\' => j += 1,
            x if x == open => depth += 1,
            x if x == close => {
                depth -= 1;
                if depth == 0 {
                    return Some(j);
                }
            }
            _ => {}
        }
        j += 1;
    }
    None
}

/// The end of a bracketed construct at `i` (a `[`): a wikilink `[[…]]`, a link `[text](target)` or `[text][ref]`, a
/// footnote `[^id]`, a citation `[@key, p. 3]`; plain brackets (None) unless `always` (an embed's or an inline
/// footnote's, whose `!` or `^` comes before).
fn bracket_end(s: &str, i: usize, always: bool) -> Option<usize> {
    if s[i..].starts_with("[[") {
        return s[i + 2..].find("]]").map(|j| i + 2 + j + 2);
    }
    let close = matching(s, i, b'[', b']')?;
    let after = close + 1;
    if s[after..].starts_with('(') {
        if let Some(j) = matching(s, after, b'(', b')') {
            return Some(j + 1);
        }
    }
    if s[after..].starts_with('[') {
        if let Some(j) = matching(s, after, b'[', b']') {
            return Some(j + 1);
        }
    }
    let inner = s[i + 1..close].trim_start();
    (always || inner.starts_with('^') || inner.starts_with('@') || inner.starts_with("-@")).then_some(after)
}

// ----- the markers

#[derive(Clone, Copy, Debug)]
struct Piece {
    a: usize,
    e: usize,
    colour: usize,
    phrase: bool,
    depth: u32,
}

impl Piece {
    /// [a, e) without the whitespace at its ends, if anything is left.
    fn trimmed(text: &str, a: usize, e: usize, m: &Mark) -> Option<Piece> {
        let (a, e) = trim(text, a, e);
        (a < e).then_some(Piece { a, e, colour: m.colour, phrase: m.phrase, depth: 0 })
    }
}

fn trim(text: &str, a: usize, e: usize) -> (usize, usize) {
    let s = &text[a..e];
    let lead = s.len() - s.trim_start().len();
    let tail = s.len() - s.trim_end().len();
    if lead == s.len() { (a, a) } else { (a + lead, e - tail) }
}

/// Pieces of one colour that overlap or touch become one.
fn merge(mut pieces: Vec<Piece>) -> Vec<Piece> {
    pieces.sort_by_key(|p| (p.colour, p.a, p.e));
    let mut out: Vec<Piece> = Vec::with_capacity(pieces.len());
    for p in pieces {
        match out.last_mut() {
            Some(q) if q.colour == p.colour && p.a <= q.e => q.e = q.e.max(p.e),
            _ => out.push(p),
        }
    }
    out
}

/// The pieces as a tree: in order, each with its depth; one that would cross an open one is split at its end.
fn nest(text: &str, mut list: Vec<Piece>) -> Vec<Piece> {
    let key = |p: &Piece| (p.a, Reverse(p.e), p.phrase, p.colour);
    list.sort_by_key(key);
    let mut out: Vec<Piece> = Vec::with_capacity(list.len());
    let mut stack: Vec<usize> = Vec::new();
    let mut i = 0;
    while i < list.len() {
        let mut p = list[i];
        i += 1;
        while stack.last().is_some_and(|&t| out[t].e <= p.a) {
            stack.pop();
        }
        if let Some(&t) = stack.last() {
            let end = out[t].e;
            if end < p.e {
                let (ra, re) = trim(text, end, p.e);
                if ra < re {
                    let rest = Piece { a: ra, e: re, ..p };
                    let at = i + list[i..].partition_point(|q| key(q) <= key(&rest));
                    list.insert(at, rest);
                }
                (p.a, p.e) = trim(text, p.a, end);
            }
        }
        if p.a >= p.e {
            continue;
        }
        p.depth = stack.len() as u32;
        out.push(p);
        stack.push(out.len() - 1);
    }
    out
}

fn tilde_label(s: &str) -> String {
    s.chars().filter(|c| !c.is_control() && *c != '~').map(|c| match c {
        '{' => '(',
        '}' => ')',
        c => c,
    }).collect::<String>().trim().to_string()
}

fn attr(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars().filter(|c| !c.is_control()) {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

fn open(c: &Named, format: Format) -> String {
    match format {
        Format::Tilde => format!("~={{{}, {}}}", tilde_label(&c.colour), tilde_label(&c.name)),
        Format::Html => format!("<mark data-colour=\"{}\" data-category=\"{}\">", attr(c.colour.trim()), attr(c.name.trim())),
    }
}

fn close(format: Format) -> &'static str {
    match format {
        Format::Tilde => "=~",
        Format::Html => "</mark>",
    }
}

/// Whether `&` at `i` starts an entity reference (`&amp;`, `&#39;`, `&#x27;`), which Markdown already reads as one.
fn entity_at(s: &str, i: usize) -> bool {
    let rest = &s[i + 1..];
    let body = rest.strip_prefix('#').map(|r| r.strip_prefix(['x', 'X']).unwrap_or(r)).unwrap_or(rest);
    let n = body.bytes().take_while(|b| b.is_ascii_alphanumeric()).count();
    n > 0 && body[n..].starts_with(';')
}

struct Out {
    s: String,
    format: Format,
    last_marker: bool,
}

impl Out {
    /// Append, with a separator where a `~=` marker would touch a `~` or `=` (see the module's notes).
    fn push(&mut self, piece: &str, marker: bool) {
        if piece.is_empty() {
            return;
        }
        if self.format == Format::Tilde {
            let tilde = |c: char| c == '~' || c == '=';
            if let (Some(x), Some(y)) = (self.s.chars().next_back(), piece.chars().next()) {
                let both_marker_tildes = x == '~' && y == '~';
                if both_marker_tildes || (marker != self.last_marker && tilde(x) && tilde(y)) {
                    self.s.push_str(SEPARATOR);
                }
            }
        }
        self.s.push_str(piece);
        self.last_marker = marker;
    }
}

/// The text [range) with the pieces (all inside it) as markers.
fn emit(text: &str, pieces: &[Piece], colours: &[Named], format: Format, atoms: &[(usize, usize)], (start, end): (usize, usize)) -> String {
    // (position, closes before opens, inner closes first and outer opens first, piece)
    let mut events: Vec<(usize, u8, u32, usize)> = Vec::with_capacity(pieces.len() * 2);
    for (i, p) in pieces.iter().enumerate() {
        events.push((p.a, 1, p.depth, i));
        events.push((p.e, 0, u32::MAX - p.depth, i));
    }
    events.sort_unstable();
    let mut out = Out { s: String::with_capacity(end - start + pieces.len() * 48), format, last_marker: false };
    let (mut pos, mut open_now) = (start, 0u32);
    for (at, kind, _, i) in events {
        if at > pos {
            let seg = text_out(text, pos, at, format == Format::Html && open_now > 0, atoms);
            out.push(&seg, false);
            pos = at;
        }
        if kind == 1 {
            out.push(&open(&colours[pieces[i].colour], format), true);
            open_now += 1;
        } else {
            out.push(close(format), true);
            open_now -= 1;
        }
    }
    if pos < end {
        let seg = text_out(text, pos, end, false, atoms);
        out.push(&seg, false);
    }
    out.s
}

/// The text [a, e), with `<`, `>` and a bare `&` written as entities when it is inside an HTML marker (outside the
/// atoms: code, links, tags; an escaped character as it is). Markdown shows the entities as the characters, so the
/// note reads the same and the markers stay well-formed HTML.
fn text_out(text: &str, a: usize, e: usize, escape: bool, atoms: &[(usize, usize)]) -> String {
    let seg = &text[a..e];
    if !escape || !seg.contains(['<', '>', '&']) {
        return seg.to_string();
    }
    let mut out = String::with_capacity(seg.len() + 16);
    let mut k = atoms.partition_point(|&(_, t)| t <= a);
    let mut chars = seg.char_indices().peekable();
    while let Some((j, c)) = chars.next() {
        let at = a + j;
        while k < atoms.len() && atoms[k].1 <= at {
            k += 1;
        }
        let in_atom = k < atoms.len() && atoms[k].0 <= at;
        if c == '\\' {
            out.push(c);
            if let Some((_, d)) = chars.next() {
                out.push(d);
            }
            continue;
        }
        match c {
            '<' if !in_atom => out.push_str("&lt;"),
            '>' if !in_atom => out.push_str("&gt;"),
            '&' if !in_atom && !entity_at(text, at) => out.push_str("&amp;"),
            c => out.push(c),
        }
    }
    out
}

/// UTF-16 offsets (the worker's, JavaScript's) to byte offsets in the text.
struct Utf16 {
    /// Each character that is not ASCII: (its UTF-16 offset, its byte offset, its length in UTF-16 units and in
    /// bytes). Between two of them bytes and UTF-16 units go up together (ASCII).
    steps: Vec<(u32, usize, u32, usize)>,
    end: (u32, usize),
}

impl Utf16 {
    fn new(text: &str) -> Utf16 {
        let mut steps = Vec::new();
        let mut u = 0u32;
        for (b, c) in text.char_indices() {
            if !c.is_ascii() {
                steps.push((u, b, c.len_utf16() as u32, c.len_utf8()));
            }
            u += c.len_utf16() as u32;
        }
        Utf16 { steps, end: (u, text.len()) }
    }

    /// The byte at UTF-16 offset `u` (inside a surrogate pair: the character's start), at most the text's length.
    fn byte(&self, u: u32) -> usize {
        if u >= self.end.0 {
            return self.end.1;
        }
        let i = self.steps.partition_point(|s| s.0 <= u);
        match i.checked_sub(1).map(|k| self.steps[k]) {
            None => u as usize, // only ASCII before it
            Some((s16, b, w16, _)) if u < s16 + w16 => b,
            Some((s16, b, w16, w8)) => b + w8 + (u - s16 - w16) as usize,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn colours() -> Vec<Named> {
        vec![Named { colour: "#FFD2D2".into(), name: "Claims".into() }, Named { colour: "#FFF59D".into(), name: "Key terms".into() }]
    }

    /// A mark over [a, e) in bytes of an ASCII text.
    fn m(a: usize, e: usize, colour: usize) -> Mark {
        Mark { from: a as u32, to: e as u32, colour, phrase: colour == 1 }
    }

    /// A mark over the first occurrence of `what` (ASCII text).
    fn over(text: &str, what: &str, colour: usize) -> Mark {
        let a = text.find(what).unwrap_or_else(|| panic!("{what:?} not in {text:?}"));
        m(a, a + what.len(), colour)
    }

    fn all(text: &str) -> Vec<Mark> {
        vec![m(0, text.len(), 0)]
    }

    fn tilde(text: &str, marks: &[Mark]) -> String {
        write(text, marks, &colours(), Format::Tilde).0
    }

    fn html(text: &str, marks: &[Mark]) -> String {
        write(text, marks, &colours(), Format::Html).0
    }

    const C: &str = "~={#FFD2D2, Claims}";
    const K: &str = "~={#FFF59D, Key terms}";

    #[test]
    fn a_sentence_and_a_phrase_inside_it_in_both_formats() {
        let text = "Detectors fail on new data. They work at home.";
        let marks = [over(text, "Detectors fail on new data.", 0), over(text, "new data", 1)];
        assert_eq!(tilde(text, &marks), format!("{C}Detectors fail on {K}new data=~.=~ They work at home."));
        assert_eq!(
            html(text, &marks),
            "<mark data-colour=\"#FFD2D2\" data-category=\"Claims\">Detectors fail on <mark data-colour=\"#FFF59D\" data-category=\"Key terms\">new data</mark>.</mark> They work at home."
        );
        assert_eq!(write(text, &marks, &colours(), Format::Tilde).1, 2);
        assert_eq!(tilde(text, &[]), text, "nothing coloured: the text as it is");
    }

    #[test]
    fn html_escapes_attributes_and_stray_characters_but_not_code_entities_or_escapes() {
        let names = vec![Named { colour: "#F4B400".into(), name: "Says \"hi\" & <b>".into() }];
        let text = "If a < b & c &amp; d, see `x<y&z` and <sup>2</sup> \\< here.";
        let (out, n) = write(text, &all(text), &names, Format::Html);
        assert_eq!(n, 1);
        assert_eq!(
            out,
            "<mark data-colour=\"#F4B400\" data-category=\"Says &quot;hi&quot; &amp; &lt;b&gt;\">If a &lt; b &amp; c &amp; d, see `x<y&z` and <sup>2</sup> \\< here.</mark>"
        );
        // text outside any marker is never changed
        let text = "Plain a < b. Coloured.";
        assert_eq!(html(text, &[over(text, "Coloured.", 0)]), "Plain a < b. <mark data-colour=\"#FFD2D2\" data-category=\"Claims\">Coloured.</mark>");
    }

    #[test]
    fn tilde_labels_cannot_close_the_marker() {
        let names = vec![Named { colour: " #abc ".into(), name: "A {b} ~=c".into() }];
        assert_eq!(write("Text.", &all("Text."), &names, Format::Tilde).0, "~={#abc, A (b) =c}Text.=~");
    }

    #[test]
    fn code_maths_comments_and_front_matter_are_never_marked() {
        let text = "---\ntitle: x\n---\nPara one.\n```rust\nlet a = 1;\n```\n$$\nx = y\n$$\n%%\nnote\n%%\nPara two.";
        let out = tilde(text, &all(text));
        assert_eq!(out, format!("---\ntitle: x\n---\n{C}Para one.=~\n```rust\nlet a = 1;\n```\n$$\nx = y\n$$\n%%\nnote\n%%\n{C}Para two.=~"));
    }

    #[test]
    fn inline_code_maths_links_footnotes_and_citations_are_taken_whole() {
        let text = "See `a b` and $x+y$ now.";
        let a = text.find("b`").unwrap();
        let e = text.find("+y").unwrap();
        assert_eq!(tilde(text, &[m(a, e, 0)]), format!("See {C}`a b` and $x+y$=~ now."));
        let text = "Read [[target|the alias]] and [text](http://x.y/a_b) today.";
        let marks = [m(text.find("alias").unwrap(), text.find("ext]").unwrap(), 0)];
        assert_eq!(tilde(text, &marks), format!("Read {C}[[target|the alias]] and [text](http://x.y/a_b)=~ today."));
        assert_eq!(tilde(text, &[over(text, "target", 1)]), format!("Read {K}[[target|the alias]]=~ and [text](http://x.y/a_b) today."));
        let text = "A claim[^1] holds [@key, p. 3] and ^[an aside] too.";
        assert_eq!(tilde(text, &[m(0, text.find("ey,").unwrap(), 0)]), format!("{C}A claim[^1] holds [@key, p. 3]=~ and ^[an aside] too."));
        assert_eq!(tilde(text, &[over(text, "an aside", 1)]), format!("A claim[^1] holds [@key, p. 3] and {K}^[an aside]=~ too."));
        let text = "Go to https://example.org/a?b=1&c=2. Then stop.";
        assert_eq!(html(text, &[over(text, "example", 1)]), "Go to <mark data-colour=\"#FFF59D\" data-category=\"Key terms\">https://example.org/a?b=1&c=2</mark>. Then stop.");
    }

    #[test]
    fn list_markers_quotes_headings_and_line_breaks_stay_outside() {
        let text = "- first item\n  continued here\n1. second\n- [ ] a task\n> quoted line\n> [!note] Title\n## A heading";
        let out = tilde(text, &all(text));
        assert_eq!(
            out,
            format!("- {C}first item=~\n  {C}continued here=~\n1. {C}second=~\n- [ ] {C}a task=~\n> {C}quoted line=~\n> [!note] {C}Title=~\n## {C}A heading=~")
        );
        assert_eq!(tilde("- ", &all("- ")), "- ", "only structure: nothing written");
    }

    #[test]
    fn table_pipes_and_block_ids_stay_outside() {
        let text = "| a b | c \\| d |\n|---|---|\n| e | f |";
        assert_eq!(tilde(text, &all(text)), format!("| {C}a b=~ | {C}c \\| d=~ |\n|---|---|\n| {C}e=~ | {C}f=~ |"));
        let text = "A sentence. ^abc-1\n\n^fig--x\n\nNext.";
        assert_eq!(tilde(text, &all(text)), format!("{C}A sentence.=~ ^abc-1\n\n^fig--x\n\n{C}Next.=~"));
    }

    #[test]
    fn crossing_marks_nest_and_touching_markers_are_kept_apart() {
        let text = "aaaa bbbb cccc dddd";
        // a sentence colour over the first two words, a phrase over the second and third: split at the sentence's end
        let out = tilde(text, &[m(0, 9, 0), m(5, 14, 1)]);
        assert_eq!(out, format!("{C}aaaa {K}bbbb=~=~ {K}cccc=~ dddd"));
        // two colours that touch: never "=~~=", which Obsidian could read as strikethrough
        let out = tilde(text, &[m(0, 4, 0), m(4, 9, 1)]);
        assert_eq!(out, format!("{C}aaaa=~ {K}bbbb=~ cccc dddd"), "the space is outside both");
        let text = "aaaabbbb";
        let out = tilde(text, &[m(0, 4, 0), m(4, 8, 1)]);
        assert_eq!(out, format!("{C}aaaa=~<!---->{K}bbbb=~"));
        assert!(!out.contains("~~"));
        // the note's own == and ~ never touch a marker
        let text = "a ==hi== b ~x";
        let out = tilde(text, &[over(text, "==hi==", 1), over(text, "~x", 0)]);
        assert_eq!(out, format!("a {K}==hi==<!---->=~ b {C}~x=~"));
        // the same colour twice, overlapping: one marker
        assert_eq!(tilde("abcdef", &[m(0, 4, 0), m(2, 6, 0)]), format!("{C}abcdef=~"));
    }

    #[test]
    fn utf16_offsets_are_mapped_to_the_text() {
        let text = "\u{1F600} caf\u{e9} fails. Next.";
        // UTF-16: the emoji is two units, é one
        let to16 = |b: usize| text[..b].encode_utf16().count() as u32;
        let (a, e) = (text.find("caf").unwrap(), text.find(" Next").unwrap());
        let marks = [Mark { from: to16(a), to: to16(e), colour: 0, phrase: false }];
        assert_eq!(write(text, &marks, &colours(), Format::Tilde).0, format!("\u{1F600} {C}caf\u{e9} fails.=~ Next."));
        let conv = Utf16::new(text);
        assert_eq!(conv.byte(1), 0, "inside the surrogate pair: the character's start");
        assert_eq!(conv.byte(to16(text.len()) + 5), text.len());
    }

    /// The selection `what` (its first occurrence) of an ASCII text, with every line marked in the first colour.
    fn copy(text: &str, what: &str, marks: &[Mark]) -> String {
        let a = text.find(what).unwrap_or_else(|| panic!("{what:?} not in {text:?}"));
        write_range(text, marks, &colours(), Format::Tilde, a as u32, (a + what.len()) as u32).0
    }

    #[test]
    fn a_selection_keeps_markers_on_its_own_part_of_a_sentence() {
        let text = "Detectors fail on new data. They work at home. Then stop.";
        let marks = [over(text, "Detectors fail on new data.", 0), over(text, "They work at home.", 0), over(text, "new data", 1)];
        assert_eq!(copy(text, "fail on new data. They work", &marks), format!("{C}fail on {K}new data=~.=~ {C}They work=~"));
        assert_eq!(copy(text, "w da", &marks), format!("{C}{K}w da=~=~"), "inside a phrase inside a sentence");
        assert_eq!(copy(text, "Then stop.", &marks), "Then stop.", "an uncoloured sentence: as it is");
        assert_eq!(copy(text, "", &marks), "", "an empty selection");
    }

    #[test]
    fn a_selection_cutting_through_a_link_or_code_leaves_the_cut_piece_outside() {
        let text = "Read [[target|the alias]] and `a b` here.";
        let all = all(text);
        assert_eq!(copy(text, "alias]] and `a b` here.", &all), format!("alias]] {C}and `a b` here.=~"));
        assert_eq!(copy(text, "Read [[target|the", &all), format!("{C}Read=~ [[target|the"));
        assert_eq!(copy(text, "a b", &all), "a b", "inside inline code: plain");
        assert_eq!(copy(text, "[[target|the alias]]", &all), format!("{C}[[target|the alias]]=~"), "a whole link: taken whole");
    }

    #[test]
    fn a_selection_over_list_items_table_rows_and_code_blocks() {
        let text = "- first item\n- second item";
        assert_eq!(copy(text, "item\n- second", &all(text)), format!("{C}item=~\n- {C}second=~"));
        let text = "| a b | c d |\n|---|---|\n| e | f |";
        assert_eq!(copy(text, "b | c d |\n|---|---|\n| e", &all(text)), format!("{C}b=~ | {C}c d=~ |\n|---|---|\n| {C}e=~"));
        let text = "Para one.\n```\ncode here\n```\nPara two.";
        assert_eq!(copy(text, "code here", &all(text)), "code here", "inside a code block: plain");
        assert_eq!(copy(text, "one.\n```\ncode here\n```\nPara", &all(text)), format!("{C}one.=~\n```\ncode here\n```\n{C}Para=~"));
        let text = "$$\nx = y\n$$\nText.";
        assert_eq!(copy(text, "x = y", &all(text)), "x = y", "inside maths: plain");
    }

    #[test]
    fn the_original_text_is_untouched() {
        let text = String::from("- A claim [[x|y]]. ^id\n\n| a | b |");
        let copy = text.clone();
        let (out, _) = write(&text, &all(&text), &colours(), Format::Html);
        assert_eq!(text, copy);
        assert_ne!(out, text);
        // and with the markers taken out, the export is the note again
        let plain = out.replace("<mark data-colour=\"#FFD2D2\" data-category=\"Claims\">", "").replace("</mark>", "");
        assert_eq!(plain, text);
    }
}
