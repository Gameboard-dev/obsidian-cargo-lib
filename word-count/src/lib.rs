//! Word counts of the chapters, from the LaTeX the converter writes. The rules are those of the thesis's LuaLaTeX
//! counter (wordcount.lua, now in the repository's archived/lua-wordcount/), applied to the source instead of the typeset
//! pages, so that a count is known as soon as a note is converted and LaTeX needs no second run to print it: the plugin
//! writes the counts to a file the preamble reads (`counts_file` in the plugin's core), and a changed word no longer
//! changes the .aux file.
//!
//! This crate is the counter alone, with no WebAssembly or plugin dependencies, in obsidian-cargo-lib rather than in the plugin, so that Quarry's command-line tools
//! (`quarry-tools wordcount`) count with the same code instead of a copy. What Quarry adds is here too and changes no
//! count: each paragraph's words and label (`Para`, `Counts::paras`), and citations left out (`Context::cites`).
//!
//! What LaTeX would typeset is followed as a stream of characters, spaces and breaks (`Machine`), and counted the way
//! the Lua counter counted the glyphs on a page:
//! - a word is a run of characters between two spaces (or en or em dashes, `--` and `---`) that holds a letter or a
//!   digit; any character beyond ASCII counts as a letter, as it did for the Lua counter (whose T1 fonts put accented
//!   letters, and the itemize bullet, above 127); a line break inside a word (TeX's hyphenation) does not end it;
//! - only text between the start and stop macros counts (`\startwordcount{key}` ... `\stopwordcount`), outside the
//!   group of the off macro (`{\wordcountoff ...}`); the four names are settings (`Macros`);
//! - a citation's brackets, with all they hold, are one word; a narrative citation prints the authors' names (from the
//!   .bib, `Names`) and then its one-word brackets; a glossary entry prints its text (from glossary.tex, `Glossary`);
//! - maths ends the word in progress and counts nothing (the numbers of align, gather and multline rows count one word
//!   each, as the Lua counter saw them: amsmath sets them as text); figures, tables, listings count nothing;
//! - every other command and environment does what its rule says (`Rules`): the built-in table, `DEFAULT_RULES`, gives
//!   the Lua counter's counts on the converter's output, and the wordcountRules key of data.json adds to it or overrides it;
//! - a command or environment without a rule, in counted text, is a warning that says which rule to add; its
//!   arguments are then counted as text (`count_chapter`).
//!
//! Two things only the typeset page shows: a word hyphenated across a page break counted twice in the Lua counter
//! (once on each page) and counts once here, and a \ref to a missing label printed `??` (no word) and counts as one
//! word here.

use std::collections::HashMap;


// ------------------------------------------------------------------ rules

/// The built-in rules, in the syntax of the wordcountRules key of data.json (`Rules::new` reads both): one rule per line, `\name
/// [arguments] behaviour` for a command and `name [arguments] behaviour` for an environment; `%` starts a comment.
/// The arguments are xparse's letters: `s` an optional star, `o` an optional [argument], `m` a mandatory {argument},
/// and a trailing `+` repeats the `o`/`m` letters while another argument follows (\cites[p.~2]{a}{b}). The behaviours
/// are `CMD_ACTS` and `ENV_ACTS`. The table follows what the Lua counter counted with the thesis's preamble,
/// checked on tests/wordcount/ (2026-10-25): every rule there was compiled and counted by it.
pub const DEFAULT_RULES: &str = r#"% Sectioning: the title is a paragraph of its own; \paragraph and \subparagraph run into their text.
\section som heading
\subsection som heading
\subsubsection som heading
\paragraph som runin
\subparagraph som runin
% Running text: the last mandatory argument is typeset as it stands.
\emph m text
\textit m text
\textbf m text
\texttt m text
\textsc m text
\textsf m text
\textrm m text
\textup m text
\textmd m text
\textsl m text
\textnormal m text
\mbox m text
\textcolor omm text
\href omm text
\hypertarget mm text
\glslink omm text
\footnote om footnote
% Citations (biblatex): the brackets, whatever they hold, are one word; a narrative citation is the authors' names
% (from the .bib: Smith, Smith and Jones, Smith et al.) and its brackets.
\cite soom cite
\Cite soom cite
\citep soom cite
\parencite soom cite
\Parencite soom cite
\autocite soom cite
\citealp soom cite
\cites oom+ cite
\parencites oom+ cite
\citet soom names-cite
\Citet soom names-cite
\textcite soom names-cite
\Textcite soom names-cite
\textcites oom+ names-cite
\citeauthor som authors
\Citeauthor som authors
\citeyear som value
\nocite m skip
% Glossary entries (glossaries-extra) print their text; the plural adds an s.
\gls om glossary
\Gls om glossary
\glspl om glossary-plural
\Glspl om glossary-plural
% References print a number or a letter: one word, run into the characters around it.
\ref sm value
\eqref m value
\pageref sm value
\rqref m value
\hypref m value
% Typeset as maths (url): no word, and the word before it ends.
\url m math
% Nothing that counts, and the words around run on.
\label m skip
\phantomsection skip
\addcontentsline mmm skip
\FloatBarrier skip
\clearpage skip
\newpage skip
\cleardoublepage skip
\noindent skip
\indent skip
\centering skip
\raggedright skip
\raggedleft skip
\tiny skip
\scriptsize skip
\footnotesize skip
\small skip
\normalsize skip
\large skip
\Large skip
\bfseries skip
\mdseries skip
\itshape skip
\upshape skip
\slshape skip
\scshape skip
\rmfamily skip
\sffamily skip
\ttfamily skip
\normalfont skip
\em skip
\ldots skip
\dots skip
\protect skip
\relax skip
\nobreak skip
\enspace skip
\thinspace skip
\% skip
\& skip
\# skip
\$ skip
\_ skip
\{ skip
\} skip
\, skip
\; skip
\: skip
\! skip
\/ skip
\- skip
\@ skip
% Accents on the letter that follows, and letters of their own.
\' skip
\` skip
\^ skip
\" skip
\~ skip
\= skip
\. skip
\c m text
\v m text
\u m text
\H m text
\r m text
\k m text
\d m text
\b m text
\t m text
\i value
\j value
\l value
\L value
\o value
\O value
\ss value
\ae value
\AE value
\oe value
\OE value
\aa value
\AA value
% Spaces between words, and paragraph breaks.
\\ so space
\newline space
\linebreak o space
\quad space
\qquad space
\hspace sm space
\hfill space
\textendash space
\textemdash space
\par par
\vspace sm par
\smallskip par
\medskip par
\bigskip par
\item o item
% Floats: their contents count: captions and a table's cells.
\caption som caption
\handtypedcode m listing
\resizebox mmm text
\scalebox om text
\includegraphics om skip
\toprule o skip
\midrule o skip
\bottomrule o skip
\hline skip
\cline m skip
\addlinespace o skip
\endhead skip
\endfirsthead skip
\endfoot skip
\endlastfoot skip
\multicolumn mmm text
\multirow omm text
figure o count
figure* o count
table o count
table* o count
tabular m count
tabular* mm count
tabularx mm count
longtable om count
% A TikZ picture counts the text it prints, once for each pass of a \foreach loop; its drawing commands never count.
tikzpicture labels
% Code counts, word by word. A hand-typed listing's code is in a file of its own: \handtypedcode's line says how many words.
handtyped o count
Verbatim verbatim
lstlisting verbatim
minted m verbatim
verbatim verbatim
% Displayed maths: each block is one word, whatever it holds.
equation math-one
equation* math-one
displaymath math-one
align* math-one
gather* math-one
flalign* math-one
alignat* m math-one
multline* math-one
align math-one
gather math-one
flalign math-one
alignat m math-one
multline math-one
% Lists: an \item's [label] is text; without one, an enumerate's number is one word and an itemize's bullet none.
enumerate list
itemize list-plain
description list-plain
% Text set differently.
center count
flushleft count
flushright count
quote count
quotation count
"#;

/// The behaviours of a command, as the rules name them (`text=2` takes the second argument instead of the last).
pub const CMD_ACTS: &[(&str, &str)] = &[
    ("text", "its last mandatory argument (or the n-th argument: text=n) is running text"),
    ("heading", "the same, as a paragraph of its own (\\section)"),
    ("runin", "the same, then a space, so that the paragraph runs on (\\paragraph)"),
    ("footnote", "the argument is counted as a paragraph of its own (the mark is not a word)"),
    ("one", "a citation: one word, whatever it holds, separate from the word before it"),
    ("cite", "a citation as printed: one word per work, and the words of its notes ([3, p. 27] is three)"),
    ("names-cite", "a narrative citation as printed: the authors' names, then the citation as printed"),
    ("caption", "a caption as printed: its label (Figure 3:) is two words, then its text"),
    ("listing", "a listing's code, kept in a file: the words the comment after it gives (%code-words n)"),
    ("names", "a narrative citation: the authors' names from the .bib, then one word for the brackets"),
    ("authors", "the authors' names only (\\citeauthor)"),
    ("value", "prints a number or a letter: one word, run into the characters around it"),
    ("glossary", "prints the glossary entry's text"),
    ("glossary-plural", "the same, with an s"),
    ("skip", "prints nothing that counts; the words around it run on"),
    ("space", "a space between words"),
    ("math", "typeset as maths: no word, and the word before it ends"),
    ("item", "a list item: its [label] is text; without one, the list's own label"),
    ("par", "ends the paragraph"),
    ("start", "starts a counted section: the argument is its key"),
    ("print", "prints a count (not counted)"),
    ("stop", "ends the counted sections"),
    ("off", "the rest of the group is not counted"),
];

/// The behaviours of an environment.
pub const ENV_ACTS: &[(&str, &str)] = &[
    ("count", "its contents are running text"),
    ("skip", "nothing in it counts"),
    ("math", "displayed maths: nothing counts"),
    ("math-one", "displayed maths that counts as one word, whatever it holds"),
    ("math-rows", "displayed maths whose numbered rows count one word each"),
    ("math-once", "displayed maths whose number, if it has one, is one word"),
    ("list", "a list whose \\item without a [label] counts one word (a number, a bullet)"),
    ("list-plain", "a list whose \\item without a [label] counts nothing"),
    ("verbatim", "its lines are counted as they stand, word by word"),
    ("labels", "a TikZ picture: only the text of its nodes and labels counts, each as a paragraph of its own"),
];

#[derive(Clone, Copy, Debug, PartialEq)]
enum Arg {
    Star,
    Opt,
    Req,
}

/// A command's or environment's arguments: xparse's `s`, `o`, `m`, and `+` to repeat.
#[derive(Clone, Debug, Default, PartialEq)]
struct Spec {
    args: Vec<Arg>,
    repeat: bool,
}

/// What a command does. The argument index (from 0) of `Text`, `Heading`, `RunIn` and `Footnote` is None for the last
/// mandatory argument.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Act {
    Text(Option<usize>),
    Heading(Option<usize>),
    RunIn(Option<usize>),
    Footnote(Option<usize>),
    One,
    Cite,
    NamesCite,
    Caption,
    Listing,
    Names,
    Authors,
    Value,
    Glossary { plural: bool },
    Skip,
    Space,
    Math,
    Item,
    Par,
    Start,
    Print,
    Stop,
    Off,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum EnvAct {
    Count,
    Skip,
    Math,
    MathRows,
    MathOnce,
    MathOne,
    List { counted: bool },
    Verbatim,
    Labels,
}

/// The counter's own macros, as the plugin's settings name them (without the backslash).
#[derive(Clone, Debug, PartialEq)]
pub struct Macros {
    pub start: String,
    pub print: String,
    pub stop: String,
    pub off: String,
}

impl Default for Macros {
    fn default() -> Self {
        Macros { start: "startwordcount".into(), print: "printwordcount".into(), stop: "stopwordcount".into(), off: "wordcountoff".into() }
    }
}

/// The rules: the built-in table, the counter's macros, and the user's rules over them.
#[derive(Clone, Debug)]
pub struct Rules {
    cmds: HashMap<String, (Spec, Act)>,
    envs: HashMap<String, (Spec, EnvAct)>,
    /// Inline maths (`$...$`, `\(...\)`) counts as one word (the plugin's setting, on by default, as Better Word
    /// Count's switch, user 2026-10-27); off, it counts nothing, as the Lua counter did.
    pub inline_math_word: bool,
    /// A `$$ ... $$` block counts as one word (the environments have their own rules).
    pub display_math_word: bool,
}

impl Default for Rules {
    fn default() -> Self {
        Rules::new("", &Macros::default()).0
    }
}

impl Rules {
    /// The built-in rules, the macros' and then `user`'s (data.json's wordcountRules), each adding a rule or replacing
    /// the one of the same name; and every line of `user` that is not a rule, as a message for the log.
    pub fn new(user: &str, m: &Macros) -> (Rules, Vec<String>) {
        let mut r = Rules { cmds: HashMap::new(), envs: HashMap::new(), inline_math_word: false, display_math_word: true };
        let built_in = r.add(DEFAULT_RULES, "built-in");
        debug_assert!(built_in.is_empty(), "{built_in:?}");
        let own = format!("\\{} m start\n\\{} m print\n\\{} stop\n\\{} off\n", m.start, m.print, m.stop, m.off);
        let mut errors = r.add(&own, "word-count macro");
        errors.extend(r.add(user, "wordcountRules"));
        (r, errors)
    }

    fn add(&mut self, text: &str, what: &str) -> Vec<String> {
        let mut errors = Vec::new();
        for (k, line) in text.lines().enumerate() {
            match parse_rule(line) {
                Ok(None) => {}
                Ok(Some(Rule::Cmd(name, spec, act))) => {
                    self.cmds.insert(name, (spec, act));
                }
                Ok(Some(Rule::Env(name, spec, act))) => {
                    self.envs.insert(name, (spec, act));
                }
                Err(e) => errors.push(format!("{what}, line {}: {e}: {:?}", k + 1, line.trim())),
            }
        }
        errors
    }
}

enum Rule {
    Cmd(String, Spec, Act),
    Env(String, Spec, EnvAct),
}

/// One line of rules: None for a blank or comment line. A `%` starts a comment, except in the name `\%`.
fn parse_rule(line: &str) -> Result<Option<Rule>, String> {
    let cut = line.char_indices().find(|&(k, c)| c == '%' && !line[..k].ends_with('\\')).map_or(line.len(), |(k, _)| k);
    let line = line[..cut].trim();
    if line.is_empty() {
        return Ok(None);
    }
    let f: Vec<&str> = line.split_whitespace().collect();
    let (name, spec, act) = match f.len() {
        2 => (f[0], "", f[1]),
        3 => (f[0], f[1], f[2]),
        _ => return Err("a rule is a name, its arguments (optional) and a behaviour, separated by spaces".into()),
    };
    let spec = parse_spec(spec)?;
    let (word, n) = match act.split_once('=') {
        Some((w, n)) => (w, Some(n.parse::<usize>().ok().filter(|&n| n >= 1).ok_or("the n of text=n is an argument number, from 1")?)),
        None => (act, None),
    };
    if let Some(cmd) = name.strip_prefix('\\') {
        let symbol = cmd.chars().count() == 1 && !cmd.starts_with(|c: char| c.is_ascii_alphabetic());
        if !symbol && (cmd.is_empty() || !cmd.bytes().all(|b| b.is_ascii_alphabetic() || b == b'@')) {
            return Err(format!("{name} is not a command name (letters, or one other character)"));
        }
        if n.is_some() && !matches!(word, "text" | "heading" | "runin" | "footnote") {
            return Err(format!("only text, heading, runin and footnote take =n, not {word}"));
        }
        let index = match n {
            None => None,
            Some(n) if n <= spec.args.len() && spec.args[n - 1] != Arg::Star => Some(n - 1),
            Some(n) => return Err(format!("{name} has no argument {n} that holds text")),
        };
        let reads_arg = matches!(word, "text" | "heading" | "runin" | "footnote" | "names" | "authors" | "glossary" | "glossary-plural" | "start");
        if reads_arg && index.is_none() && !spec.args.contains(&Arg::Req) {
            return Err(format!("{word} reads a mandatory argument: give it an m"));
        }
        let act = match word {
            "text" => Act::Text(index),
            "heading" => Act::Heading(index),
            "runin" => Act::RunIn(index),
            "footnote" => Act::Footnote(index),
            "one" => Act::One,
            "cite" => Act::Cite,
            "names-cite" => Act::NamesCite,
            "caption" => Act::Caption,
            "listing" => Act::Listing,
            "names" => Act::Names,
            "authors" => Act::Authors,
            "value" => Act::Value,
            "glossary" => Act::Glossary { plural: false },
            "glossary-plural" => Act::Glossary { plural: true },
            "skip" => Act::Skip,
            "space" => Act::Space,
            "math" => Act::Math,
            "item" => Act::Item,
            "par" => Act::Par,
            "start" => Act::Start,
            "print" => Act::Print,
            "stop" => Act::Stop,
            "off" => Act::Off,
            _ => return Err(format!("{word} is not a command behaviour ({})", names_of(CMD_ACTS))),
        };
        return Ok(Some(Rule::Cmd(cmd.to_string(), spec, act)));
    }
    if !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'*' || b == b'@') {
        return Err(format!("{name} is not an environment name (or a command: start it with \\)"));
    }
    if n.is_some() {
        return Err("an environment's behaviour takes no =n".into());
    }
    let act = match word {
        "count" => EnvAct::Count,
        "skip" => EnvAct::Skip,
        "math" => EnvAct::Math,
        "math-rows" => EnvAct::MathRows,
        "math-once" => EnvAct::MathOnce,
        "math-one" => EnvAct::MathOne,
        "list" => EnvAct::List { counted: true },
        "list-plain" => EnvAct::List { counted: false },
        "verbatim" => EnvAct::Verbatim,
        "labels" => EnvAct::Labels,
        _ => return Err(format!("{word} is not an environment behaviour ({})", names_of(ENV_ACTS))),
    };
    Ok(Some(Rule::Env(name.to_string(), spec, act)))
}

fn names_of(acts: &[(&str, &str)]) -> String {
    acts.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(", ")
}

fn parse_spec(s: &str) -> Result<Spec, String> {
    let (letters, repeat) = match s.strip_suffix('+') {
        Some(l) => (l, true),
        None => (s, false),
    };
    let args = letters
        .chars()
        .map(|c| match c {
            's' => Ok(Arg::Star),
            'o' => Ok(Arg::Opt),
            'm' => Ok(Arg::Req),
            _ => Err(format!("{s} is not an argument list (s, o and m, and + at the end to repeat)")),
        })
        .collect::<Result<Vec<_>, _>>()?;
    if repeat && args.is_empty() {
        return Err("+ repeats the letters before it".into());
    }
    Ok(Spec { args, repeat })
}

// ------------------------------------------------------------------ the .bib and the glossary

/// What a narrative citation prints for each .bib entry: the authors' family names as biblatex cites them with
/// maxcitenames=2 (`Smith`, `Smith and Jones`, `Smith et al.`; a lower-case prefix such as `van der` is left out, as
/// biblatex does without useprefix), from shortauthor, author, shorteditor, editor or translator, the first that is
/// there; and the title, which an entry without names prints instead (shorttitle, or title).
#[derive(Clone, Debug, Default)]
pub struct Names {
    entries: HashMap<String, (String, String)>,
}

impl Names {
    /// The entries of a .bib file (Better BibLaTeX's export, or any BibTeX).
    pub fn from_bib(bib: &str) -> Names {
        let mut entries = HashMap::new();
        let b = bib.as_bytes();
        let mut i = 0;
        while let Some(at) = bib[i..].find('@').map(|k| k + i) {
            i = at + 1;
            let kind_end = at + 1 + bib[at + 1..].bytes().take_while(|c| c.is_ascii_alphanumeric()).count();
            let kind = bib[at + 1..kind_end].to_ascii_lowercase();
            let Some(open) = bib[kind_end..].find(['{', '(']).map(|k| k + kind_end) else { break };
            if !bib[kind_end..open].trim().is_empty() || matches!(kind.as_str(), "comment" | "string" | "preamble" | "") {
                continue;
            }
            let end = if b[open] == b'{' { braced_end(bib, open) } else { bib[open..].find(')').map_or(bib.len(), |k| open + k + 1) };
            let body = &bib[open + 1..end.saturating_sub(1).max(open + 1)];
            i = end;
            let Some((key, fields)) = body.split_once(',') else { continue };
            let f = bib_fields(fields);
            let get = |k: &str| f.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
            let names = ["shortauthor", "author", "shorteditor", "editor", "translator"].iter().find_map(|k| get(k)).map(cited_names).unwrap_or_default();
            let title = get("shorttitle").or(get("title")).unwrap_or("").to_string();
            entries.insert(key.trim().to_string(), (names, title));
        }
        Names { entries }
    }

    /// (names, title) of an entry.
    fn get(&self, key: &str) -> Option<&(String, String)> {
        self.entries.get(key)
    }
}

/// `name = {value}`, `name = "value"`, `name = 2020`, separated by commas: (lower-case name, value).
fn bib_fields(s: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i < s.len() {
        let Some(eq) = s[i..].find('=').map(|k| k + i) else { break };
        let name = s[i..eq].trim().trim_start_matches(',').trim().to_ascii_lowercase();
        let mut j = eq + 1;
        while j < s.len() && b[j].is_ascii_whitespace() {
            j += 1;
        }
        let (value, end) = match b.get(j) {
            Some(b'{') => {
                let e = braced_end(s, j);
                (&s[j + 1..e.saturating_sub(1).max(j + 1)], e)
            }
            Some(b'"') => {
                let e = s[j + 1..].find('"').map_or(s.len(), |k| k + j + 1);
                (&s[j + 1..e], (e + 1).min(s.len()))
            }
            _ => {
                let e = s[j..].find(',').map_or(s.len(), |k| k + j);
                (s[j..e].trim(), e)
            }
        };
        out.push((name, value.to_string()));
        i = s[end..].find(',').map_or(s.len(), |k| k + end + 1);
    }
    out
}

/// A name list as a narrative citation prints it: one family name, two joined by `and`, or the first and `et al.` for
/// three or more (or for `and others`).
fn cited_names(list: &str) -> String {
    let names = split_names(list);
    let others = names.last().is_some_and(|n| n.trim() == "others");
    let fams: Vec<String> = names.iter().filter(|n| n.trim() != "others").map(|n| family(n)).collect();
    match (fams.len(), others) {
        (0, _) => String::new(),
        (1, false) => fams[0].clone(),
        (2, false) => format!("{} and {}", fams[0], fams[1]),
        _ => format!("{} et al.", fams[0]),
    }
}

/// The names of a list, split at `and` outside braces.
fn split_names(list: &str) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut depth = 0;
    let words: Vec<&str> = list.split_whitespace().collect();
    for w in words {
        if depth == 0 && w.eq_ignore_ascii_case("and") {
            out.push(String::new());
            continue;
        }
        depth += w.matches('{').count() as i32 - w.matches('}').count() as i32;
        let cur = out.last_mut().unwrap();
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(w);
    }
    out.into_iter().filter(|n| !n.is_empty()).collect()
}

/// A name's family part without its lower-case prefix (BibTeX's "von"): `van der Berg, Jan` -> `Berg`, `Jan van der
/// Berg` -> `Berg`, `{World Health Organization}` -> `World Health Organization`, `Le Roux, Paul` -> `Le Roux`.
fn family(name: &str) -> String {
    let name = name.trim();
    let first_part = match top_level_comma(name) {
        Some(c) => &name[..c],
        None => name,
    };
    let words = split_top(first_part);
    let lower = |w: &str| w.chars().find(|c| c.is_alphabetic() || *c == '{').is_some_and(|c| c.is_lowercase());
    let fam: Vec<&str> = if top_level_comma(name).is_some() {
        // "von Last": the family is what follows the last lower-case word
        match words.iter().rposition(|w| lower(w)) {
            Some(k) if k + 1 < words.len() => words[k + 1..].to_vec(),
            _ => words.clone(),
        }
    } else {
        // "First von Last": the last word, and the words after a lower-case one
        match words.iter().position(|w| lower(w)).filter(|&k| k + 1 < words.len()) {
            Some(_) => {
                let last_lower = words.iter().rposition(|w| lower(w)).unwrap();
                if last_lower + 1 < words.len() { words[last_lower + 1..].to_vec() } else { words[words.len() - 1..].to_vec() }
            }
            None => words.last().map(|w| vec![*w]).unwrap_or_default(),
        }
    };
    let out = fam.join(" ");
    let inner = out.strip_prefix('{').and_then(|s| s.strip_suffix('}'));
    match inner {
        Some(s) if !s.contains(['{', '}']) => s.to_string(),
        _ => out,
    }
}

fn top_level_comma(s: &str) -> Option<usize> {
    let mut depth = 0;
    for (k, c) in s.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => depth -= 1,
            ',' if depth == 0 => return Some(k),
            _ => {}
        }
    }
    None
}

/// Words separated by spaces outside braces: `{World Health Organization}` is one word.
fn split_top(s: &str) -> Vec<&str> {
    let (mut out, mut depth, mut start) = (Vec::new(), 0, None);
    for (k, c) in s.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => depth -= 1,
            _ => {}
        }
        if c.is_whitespace() && depth == 0 {
            if let Some(st) = start.take() {
                out.push(&s[st..k]);
            }
        } else if start.is_none() {
            start = Some(k);
        }
    }
    if let Some(st) = start {
        out.push(&s[st..]);
    }
    out
}

/// What \gls prints for each glossary entry: `\term[text]{key}{name}{long form}{description}` in the glossary's .tex
/// prints the text, or the name where there is none (the preamble's \term).
#[derive(Clone, Debug, Default)]
pub struct Glossary {
    text: HashMap<String, String>,
}

impl Glossary {
    /// Adds each entry of `other`, replacing one of the same key (a later glossary file wins).
    pub fn extend(&mut self, other: &Glossary) {
        self.text.extend(other.text.iter().map(|(k, v)| (k.clone(), v.clone())));
    }

    /// Adds the entry `key`, printed as `text` (an acronym list's short form), replacing one of the same key.
    pub fn insert(&mut self, key: &str, text: &str) {
        self.text.insert(key.to_string(), text.to_string());
    }

    /// The entries of the glossary's .tex; `term` is the entry command (the Glossary entry command setting, `term`).
    pub fn from_tex(tex: &str, term: &str) -> Glossary {
        let mut text = HashMap::new();
        let cmd = format!("\\{}", term.trim().trim_start_matches('\\'));
        let mut i = 0;
        while let Some(k) = tex[i..].find(&cmd).map(|k| k + i) {
            i = k + cmd.len();
            if tex[i..].starts_with(|c: char| c.is_ascii_alphabetic()) {
                continue; // a longer command
            }
            let (args, end) = args_at(tex, i, &Spec { args: vec![Arg::Opt, Arg::Req, Arg::Req], repeat: false });
            i = end;
            let span = |a: &Option<(usize, usize)>| a.map(|(s, e)| tex[s..e].to_string());
            if let (Some(key), Some(name)) = (span(&args[1]), span(&args[2])) {
                text.insert(key, span(&args[0]).unwrap_or(name));
            }
        }
        Glossary { text }
    }
}

// ------------------------------------------------------------------ counting

/// What the counter reads besides a chapter. `cites`: whether citations count (the plugin: always; Quarry's wordcount
/// command: unless the preamble switches them off with `\AtEveryCite{\wordcountoff}`); when false, the citation
/// behaviours (`one`, `names`, `authors`) count nothing.
pub struct Context<'a> {
    pub rules: &'a Rules,
    pub names: &'a Names,
    pub glossary: &'a Glossary,
    pub cites: bool,
}

/// A paragraph of the counted text, as Quarry's `wordcount --paragraphs` lists it: the label of the `% P3.2 | ...`
/// comment above it (`heading` for a heading, `-` when it has none), its words as counted (a citation is `[0]`), their
/// number, and whether it is inside a float (a figure's or table's caption, a table's cells: `FLOATS`). A footnote's
/// words join the paragraph of its mark. The paragraphs' counts add up to the sections' (but for a section counted
/// again from 0, `Act::Start`).
#[derive(Clone, Debug, PartialEq)]
pub struct Para {
    pub label: String,
    pub text: String,
    pub n: usize,
    pub float: bool,
}

/// A chapter's counts, by section key in the order of the chapter, its paragraphs, and its warnings: (.tex line from 1,
/// message).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Counts {
    pub sections: Vec<(String, usize)>,
    pub paras: Vec<Para>,
    pub warnings: Vec<(u32, String)>,
}

/// The environments whose paragraphs are `Para::float`.
pub const FLOATS: &[&str] = &["figure", "figure*", "table", "table*", "longtable", "wrapfigure", "wraptable", "sidewaysfigure", "sidewaystable"];

fn counter<'a>(cx: &'a Context<'a>, tex: &str) -> Counter<'a> {
    Counter {
        cx,
        m: Machine::default(),
        keys: Vec::new(),
        key: None,
        groups: vec![Group::default()],
        line_starts: std::iter::once(0).chain(tex.match_indices('\n').map(|(k, _)| k + 1)).collect(),
        unknown: Vec::new(),
        warnings: Vec::new(),
        paras: Vec::new(),
        para_from: 0,
        label: String::new(),
        heading: false,
        in_note: 0,
    }
}

/// Counts a chapter (a .tex the converter wrote).
pub fn count_chapter(tex: &str, cx: &Context) -> Counts {
    let tex = tex.replace("\r\n", "\n");
    let mut c = counter(cx, &tex);
    c.run(Src { text: &tex, base: 0, at: None, from: "" });
    c.record(true);
    let mut warnings = c.warnings;
    for (name, line, times, from) in c.unknown {
        let times = if times > 1 { format!(" ({times} times from here)") } else { String::new() };
        warnings.push((line, format!("{name}{from} has no word-count rule, so what it holds was counted as text{times}: add a rule for {name} to wordcountRules in the plugin's data.json")));
    }
    warnings.sort_by_key(|w| w.0);
    Counts { sections: c.keys.into_iter().zip(c.m.counts).collect(), paras: c.paras, warnings }
}

/// The Lua counter's page walk, on a stream instead of a page: `inword` while inside a word already counted; a space
/// ends the word only when the next character arrives (`gap`), maths and dashes end it at once, a paragraph's end too.
#[derive(Default)]
struct Machine {
    inword: bool,
    gap: bool,
    counts: Vec<usize>,
    /// The words counted, with their section: the paragraphs' text (`Para`), and the list to compare with the Lua
    /// counter's (tests).
    trace: Vec<(usize, String)>,
}

impl Machine {
    /// A character, counted for `key` (None: outside the counted text) if it starts a word.
    fn glyph(&mut self, c: char, key: Option<usize>) {
        if self.gap {
            self.inword = false;
            self.gap = false;
        }
        if is_dash(c) {
            self.inword = false;
        } else if !self.inword && is_letter(c) {
            if let Some(k) = key {
                self.counts[k] += 1;
                self.inword = true;
                self.trace.push((k, String::new()));
            }
        }
        if self.inword {
            if let Some(w) = self.trace.last_mut() {
                w.1.push(c);
            }
        }
    }

    /// A citation's brackets: one word, whatever they hold, and the characters directly after them belong to it.
    fn cite(&mut self, key: Option<usize>) {
        if self.gap {
            self.gap = false;
        }
        if let Some(k) = key {
            self.counts[k] += 1;
            self.trace.push((k, CITE.into()));
        }
        self.inword = true;
    }

    fn space(&mut self) {
        self.gap = true;
    }

    fn math(&mut self) {
        self.inword = false;
    }

    fn par(&mut self) {
        self.inword = false;
        self.gap = false;
    }
}

/// A citation in `Machine::trace`.
const CITE: &str = "<CITE>";

/// A letter or digit to the Lua counter: ASCII letters and digits, and everything beyond ASCII but the dashes.
fn is_letter(c: char) -> bool {
    c.is_ascii_alphanumeric() || (c as u32 >= 128 && !is_dash(c))
}

fn is_dash(c: char) -> bool {
    c == '\u{2013}' || c == '\u{2014}'
}

/// A group's state: the off macro's flag, and the list it is in (whether an \item without a label counts).
#[derive(Clone, Debug, Default)]
struct Group {
    off: bool,
    list: Option<bool>,
    env: Option<String>,
}

/// Text being read: the chapter or a slice of it (`base`, its offset in the chapter, gives line numbers), or text from
/// elsewhere (a glossary entry, a .bib name), whose warnings name the chapter line `at` and say where it came `from`.
#[derive(Clone, Copy)]
struct Src<'s> {
    text: &'s str,
    base: usize,
    at: Option<u32>,
    from: &'s str,
}

impl<'s> Src<'s> {
    fn sub(&self, s: usize, e: usize) -> Src<'s> {
        Src { text: &self.text[s..e], base: self.base + s, ..*self }
    }
}

struct Counter<'a> {
    cx: &'a Context<'a>,
    m: Machine,
    keys: Vec<String>,
    key: Option<usize>,
    groups: Vec<Group>,
    line_starts: Vec<usize>,
    /// Commands and environments without a rule: (name, first line, times, where from).
    unknown: Vec<(String, u32, usize, String)>,
    warnings: Vec<(u32, String)>,
    paras: Vec<Para>,
    /// Where the paragraph being read starts in `m.trace`.
    para_from: usize,
    /// The label of the paragraph being read (a `% P3.2 | ...` comment above it).
    label: String,
    /// Inside a heading's title.
    heading: bool,
    /// Inside footnotes: their words join the paragraph of their mark.
    in_note: usize,
}

impl Counter<'_> {
    /// The section the next character counts for: None outside the counted text or in the off macro's group.
    fn mark(&self) -> Option<usize> {
        if self.groups.last().is_some_and(|g| g.off) { None } else { self.key }
    }

    fn line(&self, s: &Src, i: usize) -> u32 {
        s.at.unwrap_or_else(|| self.line_starts.partition_point(|&st| st <= s.base + i) as u32)
    }

    fn push(&mut self, env: Option<&str>) {
        let mut g = self.groups.last().cloned().unwrap_or_default();
        g.env = env.map(String::from);
        self.groups.push(g);
    }

    fn pop(&mut self) {
        if self.groups.len() > 1 {
            self.groups.pop();
        }
    }

    fn glyph(&mut self, c: char) {
        let k = self.mark();
        self.m.glyph(c, k);
    }

    /// A paragraph ends: the word in progress ends, and the paragraph is recorded (`blank`: at a blank line).
    fn par(&mut self, blank: bool) {
        self.m.par();
        self.record(blank);
    }

    /// Records the words read since the last paragraph as one (`Para`), without touching the counting state: a float's
    /// \begin and \end call it alone, so that its caption is a paragraph of its own but the count is the Lua counter's.
    /// `blank` (a blank line, or the end) also ends the label; a heading does too.
    fn record(&mut self, blank: bool) {
        if self.in_note > 0 || self.m.trace.len() == self.para_from {
            return;
        }
        let words = &self.m.trace[self.para_from..];
        let text = words.iter().map(|(_, w)| w.strip_prefix(CITE).map_or_else(|| w.clone(), |rest| format!("[0]{rest}"))).collect::<Vec<_>>().join(" ");
        let label = if self.heading { "heading".to_string() } else if self.label.is_empty() { "-".to_string() } else { self.label.clone() };
        let float = self.groups.iter().any(|g| g.env.as_deref().is_some_and(|e| FLOATS.contains(&e)));
        self.paras.push(Para { label, text, n: words.len(), float });
        self.para_from = self.m.trace.len();
        if blank || self.heading {
            self.label.clear();
        }
    }

    fn unknown(&mut self, s: &Src, i: usize, name: String) {
        if self.mark().is_none() {
            return;
        }
        let from = if s.from.is_empty() { String::new() } else { format!(" ({})", s.from) };
        if let Some(u) = self.unknown.iter_mut().find(|u| u.0 == name && u.3 == from) {
            u.2 += 1;
        } else {
            let line = self.line(s, i);
            self.unknown.push((name, line, 1, from));
        }
    }

    fn warn(&mut self, s: &Src, i: usize, msg: String) {
        let line = self.line(s, i);
        let from = if s.from.is_empty() { String::new() } else { format!(" ({})", s.from) };
        self.warnings.push((line, format!("{msg}{from}")));
    }

    /// Reads `s` to its end.
    fn run(&mut self, s: Src) {
        let t = s.text;
        let mut i = 0;
        while i < t.len() {
            let c = t[i..].chars().next().unwrap();
            match c {
                '%' => {
                    let nl = t[i..].find('\n').map_or(t.len(), |k| k + i);
                    if let Some(l) = paragraph_label(&t[i..nl]) {
                        self.label = l;
                    }
                    i = after_comment(t, i);
                    if t[i..].starts_with('\n') {
                        i = self.white(t, i, 1);
                    }
                }
                '\\' => i = self.control(s, i),
                '{' => {
                    self.push(None);
                    i += 1;
                }
                '}' => {
                    self.pop();
                    i += 1;
                }
                '$' => i = self.dollar(s, i),
                '~' => {
                    self.m.space();
                    i += 1;
                }
                ' ' | '\t' | '\r' | '\n' => i = self.white(t, i, 0),
                '-' => {
                    let n = t[i..].bytes().take_while(|&b| b == b'-').count();
                    match n {
                        1 => self.glyph('-'),
                        _ => {
                            // -- and --- are an en and an em dash, a longer run dashes and hyphens
                            for _ in 0..n / 2 {
                                self.glyph('\u{2013}');
                            }
                        }
                    }
                    i += n;
                }
                // a table's column break separates words; maths characters typeset nothing outside maths
                '&' => {
                    self.m.space();
                    i += 1;
                }
                '^' | '_' | '#' => i += 1,
                '\u{2010}' => {
                    self.glyph('-'); // the preamble prints the Unicode hyphen as -
                    i += c.len_utf8();
                }
                _ => {
                    self.glyph(c);
                    i += c.len_utf8();
                }
            }
        }
    }

    /// Spaces, tabs and line ends from `i`: a space, or the end of a paragraph if a line holds nothing (`newlines`: line
    /// ends already read, after a comment).
    fn white(&mut self, t: &str, mut i: usize, mut newlines: usize) -> usize {
        while let Some(&b) = t.as_bytes().get(i) {
            match b {
                b'\n' => newlines += 1,
                b' ' | b'\t' | b'\r' => {}
                _ => break,
            }
            i += 1;
        }
        if newlines >= 2 {
            self.par(true);
        } else {
            self.m.space();
        }
        i
    }

    /// After a control word TeX skips spaces and one line end; a line that holds nothing after it still ends the
    /// paragraph.
    fn after_word(&mut self, t: &str, i: usize) -> usize {
        let mut j = skip_blanks(t, i);
        if t[j..].starts_with('\n') {
            j = skip_blanks(t, j + 1);
            if t[j..].starts_with('\n') {
                j = self.white(t, j, 2); // a paragraph's end
            }
        }
        j
    }

    fn dollar(&mut self, s: Src, i: usize) -> usize {
        let t = s.text;
        if t[i..].starts_with("$$") {
            let end = find_unescaped(t, i + 2, "$$").map_or(t.len(), |k| k + 2);
            self.par(false);
            if self.cx.rules.display_math_word {
                self.glyph('0');
            }
            self.par(false);
            return end;
        }
        let end = find_unescaped(t, i + 1, "$").map_or(t.len(), |k| k + 1);
        self.inline_math();
        end
    }

    /// Inline maths: one word of its own when the rules say so, else nothing; either way it ends the word before it.
    fn inline_math(&mut self) {
        self.m.math();
        if self.cx.rules.inline_math_word {
            let k = self.mark();
            self.m.cite(k);
            self.m.math();
        }
    }

    /// A control sequence at `i`; returns the index after it and its arguments.
    fn control(&mut self, s: Src, i: usize) -> usize {
        let t = s.text;
        let rest = &t[i + 1..];
        let letters = rest.bytes().take_while(|b| b.is_ascii_alphabetic() || *b == b'@').count();
        let Some(first) = rest.chars().next() else { return t.len() };
        let (name, mut j) = if letters > 0 { (&rest[..letters], i + 1 + letters) } else { (&rest[..first.len_utf8()], i + 1 + first.len_utf8()) };
        match name {
            " " | "\t" | "\n" | "\r" => {
                self.m.space(); // a control space (\ ), or a backslash at the end of a line
                return if letters == 0 && name == "\n" { skip_blanks(t, j) } else { j };
            }
            "(" => {
                let end = find_unescaped(t, j, "\\)").map_or(t.len(), |k| k + 2);
                self.inline_math();
                return end;
            }
            "[" => {
                let end = find_unescaped(t, j, "\\]").map_or(t.len(), |k| k + 2);
                self.par(false);
                self.par(false);
                return end;
            }
            _ => {}
        }
        if letters > 0 {
            j = self.after_word(t, j);
        }
        match name {
            "begin" => return self.begin(s, i, j),
            "end" => return self.end(s, i, j),
            _ => {}
        }
        let Some((spec, act)) = self.cx.rules.cmds.get(name).cloned() else {
            self.unknown(&s, i, format!("\\{name}"));
            return j;
        };
        let (args, end) = args_at(t, j, &spec);
        self.act(s, i, act, &spec, &args);
        end
    }

    /// The argument a text behaviour reads: the n-th, or the last mandatory one.
    fn text_arg(spec: &Spec, args: &[Option<(usize, usize)>], n: Option<usize>) -> Option<(usize, usize)> {
        match n {
            Some(n) => args.get(n).copied().flatten(),
            None => {
                let k = (0..args.len()).rev().find(|&k| spec_arg(spec, k) == Arg::Req)?;
                args[k]
            }
        }
    }

    /// Reads an argument as text, in a group of its own.
    fn text(&mut self, s: Src, span: Option<(usize, usize)>) {
        if let Some((a, b)) = span {
            self.push(None);
            self.run(s.sub(a, b));
            self.pop();
        }
    }

    /// Text from outside the chapter (a name, a glossary entry), read as LaTeX in a group of its own.
    /// The words of a citation's optional notes, `\cite[see][p.~27]{key}`: what the brackets print besides the number.
    fn cite_notes(&mut self, s: &Src, i: usize, spec: &Spec, args: &[Option<(usize, usize)>]) {
        for (n, a) in args.iter().enumerate() {
            if let (Arg::Opt, Some((a, b))) = (spec_arg(spec, n), *a) {
                let note = s.text[a..b].to_string();
                self.m.space();
                self.foreign(s, i, &note, "a citation's note");
                self.m.space();
            }
        }
    }

    fn foreign(&mut self, s: &Src, i: usize, text: &str, from: &str) {
        let at = Some(self.line(s, i));
        self.push(None);
        self.run(Src { text, base: 0, at, from });
        self.pop();
    }

    fn act(&mut self, s: Src, i: usize, act: Act, spec: &Spec, args: &[Option<(usize, usize)>]) {
        let t = s.text;
        let req = |k: usize| args.iter().enumerate().filter(|(n, _)| spec_arg(spec, *n) == Arg::Req).filter_map(|(_, a)| *a).nth(k).map(|(a, b)| &t[a..b]);
        let last_req = || Self::text_arg(spec, args, None).map(|(a, b)| t[a..b].trim().to_string());
        if !self.cx.cites && matches!(act, Act::One | Act::Cite | Act::NamesCite | Act::Names | Act::Authors) {
            return; // \AtEveryCite{\wordcountoff}: a citation, names and all, counts nothing
        }
        match act {
            Act::Text(n) => self.text(s, Self::text_arg(spec, args, n)),
            Act::Heading(n) => {
                self.par(false);
                self.heading = true;
                self.text(s, Self::text_arg(spec, args, n));
                self.par(false);
                self.heading = false;
            }
            Act::RunIn(n) => {
                self.par(false);
                self.text(s, Self::text_arg(spec, args, n));
                self.m.space();
            }
            Act::Footnote(n) => {
                // a paragraph at the foot of the page: the word around the mark runs on
                let (inword, gap) = (self.m.inword, self.m.gap);
                self.in_note += 1;
                self.par(false);
                self.text(s, Self::text_arg(spec, args, n));
                self.par(false);
                self.in_note -= 1;
                (self.m.inword, self.m.gap) = (inword, gap);
            }
            Act::One => {
                let k = self.mark();
                self.m.cite(k);
            }
            Act::Cite => {
                let works = (0..).map_while(req).flat_map(|a| a.split(',')).filter(|k| !k.trim().is_empty()).count();
                for _ in 0..works.max(1) {
                    let k = self.mark();
                    self.m.cite(k);
                }
                self.cite_notes(&s, i, spec, args);
            }
            Act::Caption => {
                self.foreign(&s, i, "Figure 0: ", "");
                self.text(s, Self::text_arg(spec, args, None));
            }
            Act::Listing => {
                // the words the converter wrote at the end of the line: \handtypedcode{name}%code-words 12
                let after = args.iter().rev().find_map(|a| a.map(|(_, b)| b)).map_or(i, |b| b + 1).min(t.len());
                let line = t[after..].lines().next().unwrap_or("");
                let n: usize = line.trim().strip_prefix("%code-words").and_then(|v| v.trim().parse().ok()).unwrap_or(0);
                for _ in 0..n {
                    self.glyph('0');
                    self.m.space();
                }
            }
            Act::Names | Act::NamesCite | Act::Authors => {
                let keys: Vec<String> = (0..).map_while(req).flat_map(|a| a.split(',').map(|k| k.trim().to_string()).collect::<Vec<_>>()).filter(|k| !k.is_empty()).collect();
                let n = keys.len();
                for (k, key) in keys.iter().enumerate() {
                    if k > 0 {
                        let sep = if n == 2 { " and " } else if k == n - 1 { ", and " } else { ", " };
                        self.foreign(&s, i, sep, "");
                    }
                    let from = format!("the .bib entry {key}");
                    match self.cx.names.get(key) {
                        Some((names, title)) => {
                            let printed = if names.is_empty() && act != Act::Authors { title.clone() } else { names.clone() };
                            self.foreign(&s, i, &printed, &from);
                        }
                        None => {
                            self.warn(&s, i, format!("no entry {key} in the .bib: its names were counted as one word"));
                            self.glyph('0');
                        }
                    }
                    if act != Act::Authors {
                        let m = self.mark();
                        self.m.cite(m);
                    }
                }
                if act == Act::NamesCite {
                    self.cite_notes(&s, i, spec, args);
                }
            }
            Act::Value => self.glyph('0'),
            Act::Glossary { plural } => {
                let Some(key) = last_req() else { return };
                match self.cx.glossary.text.get(&key).cloned() {
                    Some(text) => self.foreign(&s, i, &text, &format!("the glossary entry {key}")),
                    None => {
                        self.warn(&s, i, format!("no glossary entry {key}: counted as one word"));
                        self.glyph('0');
                    }
                }
                if plural {
                    self.glyph('s');
                }
            }
            Act::Skip => {}
            Act::Space => self.m.space(),
            Act::Math => self.m.math(),
            Act::Par | Act::Print => {
                self.par(false);
            }
            Act::Item => {
                self.par(false);
                let label = args.iter().enumerate().find(|(k, _)| spec_arg(spec, *k) == Arg::Opt).and_then(|(_, a)| *a);
                match (label, self.groups.last().and_then(|g| g.list)) {
                    (Some(span), _) => {
                        self.text(s, Some(span));
                        self.m.space();
                    }
                    (None, Some(true)) => {
                        self.glyph('0'); // the number or the bullet
                        self.m.space();
                    }
                    _ => {}
                }
            }
            Act::Start => {
                self.par(false);
                let key = last_req().unwrap_or_default();
                let k = match self.keys.iter().position(|x| *x == key) {
                    Some(k) => {
                        self.warn(&s, i, format!("the counted section {key} starts again: its count starts from 0, as in LaTeX"));
                        self.m.counts[k] = 0;
                        k
                    }
                    None => {
                        self.keys.push(key);
                        self.m.counts.push(0);
                        self.keys.len() - 1
                    }
                };
                self.key = Some(k);
                for g in &mut self.groups {
                    g.off = false; // set globally
                }
            }
            Act::Stop => {
                self.par(false);
                self.key = None;
            }
            Act::Off => {
                if let Some(g) = self.groups.last_mut() {
                    g.off = true;
                }
            }
        }
    }

    fn env_name(t: &str, j: usize) -> Option<(String, usize)> {
        let k = skip_blanks(t, j);
        if !t[k..].starts_with('{') {
            return None;
        }
        let e = braced_end(t, k);
        Some((t[k + 1..e - 1].trim().to_string(), e))
    }

    fn begin(&mut self, s: Src, i: usize, j: usize) -> usize {
        let t = s.text;
        let Some((name, after)) = Self::env_name(t, j) else { return j };
        let (spec, act) = match self.cx.rules.envs.get(&name) {
            Some(r) => r.clone(),
            None => {
                self.unknown(&s, i, format!("the environment {name}"));
                (Spec::default(), EnvAct::Count)
            }
        };
        let (_, after) = args_at(t, after, &spec);
        match act {
            EnvAct::Count => {
                let float = FLOATS.contains(&name.as_str());
                if float {
                    self.record(false);
                }
                self.push(Some(&name));
                after
            }
            EnvAct::List { counted } => {
                self.par(false);
                self.push(Some(&name));
                self.groups.last_mut().unwrap().list = Some(counted);
                after
            }
            EnvAct::MathOne => {
                let (_, end) = env_end(t, after, &name);
                self.par(false);
                self.glyph('0');
                self.par(false);
                end
            }
            EnvAct::Skip | EnvAct::Math | EnvAct::MathRows | EnvAct::MathOnce => {
                let (body_end, end) = env_end(t, after, &name);
                if act != EnvAct::Skip {
                    self.par(false);
                    let body = &t[after..body_end];
                    let numbered = |row: &str| row.contains("\\tag") || !(row.contains("\\nonumber") || row.contains("\\notag"));
                    let n = match act {
                        EnvAct::MathRows => rows(body).into_iter().filter(|r| !r.trim().is_empty() && numbered(r)).count(),
                        EnvAct::MathOnce => usize::from(numbered(body)),
                        _ => 0,
                    };
                    for _ in 0..n {
                        self.glyph('0'); // "(3)", at the end of its line
                        self.par(false);
                    }
                    self.par(false);
                }
                end
            }
            EnvAct::Labels => {
                let (body_end, end) = env_end(t, after, &name);
                // each label as often as the picture prints it; a loop that cannot be written out counts its labels once
                let body = expand_foreach(&t[after..body_end]);
                let vars = foreach_vars(&body);
                self.par(false);
                for (a, b) in tikz_labels(&body) {
                    let mut label = body[a..b].to_string();
                    for v in &vars {
                        label = replace_command(&label, v, "0");
                    }
                    self.foreign(&s, after, &label, "a picture's label");
                    self.par(false);
                }
                end
            }
            EnvAct::Verbatim => {
                let close = format!("\\end{{{name}}}");
                let body_end = t[after..].find(&close).map_or(t.len(), |k| k + after);
                let body = t[after..body_end].strip_prefix('\n').unwrap_or(&t[after..body_end]);
                for line in body.lines() {
                    self.par(false);
                    for c in line.chars() {
                        if c == ' ' || c == '\t' {
                            self.m.space();
                        } else {
                            self.glyph(c);
                        }
                    }
                }
                self.par(false);
                (body_end + close.len()).min(t.len())
            }
        }
    }

    fn end(&mut self, s: Src, i: usize, j: usize) -> usize {
        let t = s.text;
        let Some((name, after)) = Self::env_name(t, j) else { return j };
        match self.groups.iter().rposition(|g| g.env.as_deref() == Some(name.as_str())) {
            Some(k) if k > 0 => {
                if k + 1 != self.groups.len() {
                    self.warn(&s, i, format!("\\end{{{name}}} closes groups left open inside it"));
                }
                let list = self.groups[k].list.is_some();
                if FLOATS.contains(&name.as_str()) {
                    self.record(false);
                }
                self.groups.truncate(k);
                if list {
                    self.par(false);
                }
            }
            _ => {
                if self.mark().is_some() {
                    self.warn(&s, i, format!("\\end{{{name}}} without its \\begin"));
                }
            }
        }
        after
    }
}

/// The letter of argument k of a spec, repeated by `+` (the repeated part is its `o` and `m` letters).
fn spec_arg(spec: &Spec, k: usize) -> Arg {
    if k < spec.args.len() {
        return spec.args[k];
    }
    let rep: Vec<Arg> = spec.args.iter().copied().filter(|a| *a != Arg::Star).collect();
    rep.get((k - spec.args.len()) % rep.len().max(1)).copied().unwrap_or(Arg::Req)
}

/// The arguments of a command whose name ends at `j`, by its spec: the span of each (inside its brackets), or None
/// where an optional one is absent; and the index after the last.
fn args_at(t: &str, mut j: usize, spec: &Spec) -> (Vec<Option<(usize, usize)>>, usize) {
    let mut out = Vec::new();
    let take = |j: &mut usize, a: Arg, out: &mut Vec<Option<(usize, usize)>>| -> bool {
        match a {
            Arg::Star => {
                let found = t[*j..].starts_with('*');
                out.push(found.then(|| (*j, *j + 1)));
                if found {
                    *j += 1;
                }
                found
            }
            Arg::Opt => {
                let k = skip_blanks(t, *j);
                if t[k..].starts_with('[') {
                    let e = bracket_end(t, k);
                    out.push(Some((k + 1, e.saturating_sub(1).max(k + 1))));
                    *j = e;
                    true
                } else {
                    out.push(None);
                    false
                }
            }
            Arg::Req => {
                let mut k = skip_blanks(t, *j);
                if t[k..].starts_with('\n') {
                    k = skip_blanks(t, k + 1);
                }
                let Some(c) = t[k..].chars().next() else {
                    out.push(None);
                    return false;
                };
                let e = match c {
                    '{' => {
                        let e = braced_end(t, k);
                        out.push(Some((k + 1, e.saturating_sub(1).max(k + 1))));
                        e
                    }
                    '\\' => {
                        let n = t[k + 1..].bytes().take_while(|b| b.is_ascii_alphabetic()).count().max(1);
                        let e = (k + 1 + n).min(t.len());
                        out.push(Some((k, e)));
                        e
                    }
                    _ => {
                        out.push(Some((k, k + c.len_utf8())));
                        k + c.len_utf8()
                    }
                };
                *j = e;
                true
            }
        }
    };
    for &a in &spec.args {
        take(&mut j, a, &mut out);
    }
    if spec.repeat {
        let rep: Vec<Arg> = spec.args.iter().copied().filter(|a| *a != Arg::Star).collect();
        loop {
            let k = skip_blanks(t, j);
            if !(t[k..].starts_with('[') || t[k..].starts_with('{')) {
                break;
            }
            for &a in &rep {
                take(&mut j, a, &mut out);
            }
        }
    }
    (out, j)
}

fn skip_blanks(t: &str, mut i: usize) -> usize {
    while matches!(t.as_bytes().get(i), Some(b' ' | b'\t' | b'\r')) {
        i += 1;
    }
    i
}

/// After a comment: past its line end and the next line's leading blanks (TeX drops both).
fn after_comment(t: &str, i: usize) -> usize {
    match t[i..].find('\n') {
        Some(k) => skip_blanks(t, i + k + 1),
        None => t.len(),
    }
}

/// The index after the `}` closing the `{` at `i`, past escaped braces and comments.
fn braced_end(t: &str, i: usize) -> usize {
    let b = t.as_bytes();
    let (mut depth, mut k) = (0i32, i);
    while k < b.len() {
        match b[k] {
            b'\\' => k += 1,
            b'%' => {
                k = t[k..].find('\n').map_or(b.len(), |n| k + n);
                continue;
            }
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return k + 1;
                }
            }
            _ => {}
        }
        k += 1;
    }
    b.len()
}

/// The index after the `]` closing the `[` at `i`; braces inside may hold a `]`.
fn bracket_end(t: &str, i: usize) -> usize {
    let b = t.as_bytes();
    let (mut depth, mut k) = (0i32, i + 1);
    while k < b.len() {
        match b[k] {
            b'\\' => k += 1,
            b'{' => depth += 1,
            b'}' => depth -= 1,
            b']' if depth == 0 => return k + 1,
            _ => {}
        }
        k += 1;
    }
    b.len()
}

/// The first `pat` from `i` that is not escaped by a backslash.
fn find_unescaped(t: &str, mut i: usize, pat: &str) -> Option<usize> {
    while let Some(k) = t[i..].find(pat).map(|k| k + i) {
        let backslashes = t[..k].bytes().rev().take_while(|&b| b == b'\\').count();
        if backslashes % 2 == 0 || pat.starts_with('\\') {
            return Some(k);
        }
        i = k + pat.len();
    }
    None
}

/// The text of a TikZ picture's nodes and labels, as spans of `body` inside their braces: `\node[…] (name) at (…)
/// {text}`, `node[…] {text}` on a path, `label=above:{text}` or `label={[…]above:text}`. A `node` followed by anything
/// but its options, name, `at (…)` and `{` is not a node (`node distance=…`, `every node/.style=…`).
pub fn tikz_labels(body: &str) -> Vec<(usize, usize)> {
    let b = body.as_bytes();
    let word_at = |k: usize, w: &str| {
        body[k..].starts_with(w)
            && (k == 0 || !b[k - 1].is_ascii_alphanumeric())
            && !b.get(k + w.len()).is_some_and(|c| c.is_ascii_alphanumeric())
    };
    let skip_ws = |mut k: usize| {
        while b.get(k).is_some_and(|c| c.is_ascii_whitespace()) {
            k += 1;
        }
        k
    };
    // the index after the bracket closing the one at k ([ or ()
    let closing = |k: usize| {
        let (open, close) = (b[k], if b[k] == b'[' { b']' } else { b')' });
        let mut depth = 0;
        for (n, &c) in b.iter().enumerate().skip(k) {
            if c == open {
                depth += 1;
            } else if c == close {
                depth -= 1;
                if depth == 0 {
                    return n + 1;
                }
            }
        }
        b.len()
    };
    let mut out = Vec::new();
    let mut k = 0;
    // a node's {text}, read once the scan has passed its options (which may hold label=…)
    let mut pending: Option<(usize, usize)> = None;
    while k < b.len() {
        if let Some((j, e)) = pending.filter(|&(j, _)| k >= j) {
            out.push((j + 1, e.saturating_sub(1).max(j + 1)));
            k = e;
            pending = None;
            continue;
        }
        if b[k] == b'%' {
            k = body[k..].find('\n').map_or(b.len(), |n| k + n);
            continue;
        }
        if b[k] == b'\\' {
            // \node is a node; any other command or escaped character is passed over
            k += if word_at(k + 1, "node") { 1 } else { 1 + body[k + 1..].chars().next().map_or(0, char::len_utf8) };
            continue;
        }
        if word_at(k, "node") {
            let mut j = skip_ws(k + 4);
            loop {
                match b.get(j) {
                    Some(b'[' | b'(') => j = skip_ws(closing(j)),
                    _ if word_at(j, "at") => j = skip_ws(j + 2),
                    _ => break,
                }
            }
            if b.get(j) == Some(&b'{') {
                pending = Some((j, braced_end(body, j)));
            }
            k += 4;
            continue;
        }
        if word_at(k, "label") {
            let j = skip_ws(k + 5);
            if b.get(j) == Some(&b'=') {
                let j = skip_ws(j + 1);
                let (mut s, e) = if b.get(j) == Some(&b'{') {
                    let e = braced_end(body, j);
                    (j + 1, e.saturating_sub(1).max(j + 1))
                } else {
                    (j, body[j..].find([',', ']']).map_or(b.len(), |n| j + n))
                };
                // [options] and a position (above:) come before the text
                if b.get(s) == Some(&b'[') {
                    s = closing(s);
                }
                if let Some(c) = body[s..e].find(':') {
                    if !body[s..s + c].contains('{') {
                        s += c + 1;
                    }
                }
                let s = skip_ws(s);
                if b.get(s) == Some(&b'{') && s < e {
                    let inner = braced_end(body, s).min(e + 1);
                    out.push((s + 1, inner.saturating_sub(1).max(s + 1)));
                } else if s < e {
                    out.push((s, e));
                }
                k = e.max(k + 5);
                continue;
            }
        }
        k += body[k..].chars().next().map_or(1, char::len_utf8);
    }
    out
}

const FOREACH_MAX_PASSES: usize = 1000;
const FOREACH_MAX_BYTES: i64 = 200_000;
const FOREACH_MAX_DEPTH: usize = 8;

/// The first index from `i` where `ch` stands outside `{...}` (a backslash protects the character after it).
fn find_outside_braces(s: &str, i: usize, ch: u8) -> Option<usize> {
    let b = s.as_bytes();
    let (mut i, mut depth) = (i, 0i32);
    while i < b.len() {
        match b[i] {
            b'\\' => i += 1,
            c if c == ch && depth == 0 => return Some(i),
            b'{' => depth += 1,
            b'}' => depth -= 1,
            _ => {}
        }
        i += 1;
    }
    None
}

fn split_outside_braces(s: &str, ch: u8) -> Vec<&str> {
    let (mut parts, mut start) = (Vec::new(), 0);
    while let Some(j) = find_outside_braces(s, start, ch) {
        parts.push(s[start..j].trim());
        start = j + 1;
    }
    parts.push(s[start..].trim());
    parts
}

/// A loop's values, with `0,...,10` and `0,2,...,10` written out; a range that is not numeric stays as its two ends.
fn foreach_values(list: &str) -> Vec<String> {
    let items: Vec<&str> = split_outside_braces(list, b',').into_iter().filter(|x| !x.is_empty()).collect();
    let plain = || items.iter().filter(|x| **x != "...").take(FOREACH_MAX_PASSES).map(|x| x.to_string()).collect::<Vec<_>>();
    let Some(dots) = items.iter().position(|x| *x == "...") else { return plain() };
    if dots == 0 || dots + 1 == items.len() {
        return plain();
    }
    let num = |k: usize| items[k].parse::<f64>().ok();
    let (Some(from), Some(to)) = (num(dots - 1), num(dots + 1)) else { return plain() };
    let step = match if dots >= 2 { num(dots - 2) } else { None } {
        Some(before) => from - before,
        None if to < from => -1.0,
        None => 1.0,
    };
    if step == 0.0 {
        return plain();
    }
    let mut values: Vec<String> = items[..dots].iter().map(|x| x.to_string()).collect();
    let mut k = 1.0;
    while values.len() < FOREACH_MAX_PASSES {
        let v = from + k * step;
        if (step > 0.0 && v > to + 1e-9) || (step < 0.0 && v < to - 1e-9) {
            break;
        }
        values.push(format!("{}", (v * 1e6).round() / 1e6));
        k += 1.0;
    }
    values.extend(items[dots + 2..].iter().map(|x| x.to_string()));
    values.truncate(FOREACH_MAX_PASSES);
    values
}

struct Loop {
    vars: Vec<String>,
    count: Option<String>,
    first: i64,
    values: Vec<String>,
    body: String,
    end: usize,
}

fn command_name(b: &[u8], mut j: usize) -> usize {
    while j < b.len() && b[j].is_ascii_alphabetic() {
        j += 1;
    }
    j
}

/// The `\foreach` at `at`: its variables, `[count=\k from n]`, values and body, or None if its list is not written out
/// here. The body is a `{...}` group, or one statement up to its `;`.
fn parse_foreach(code: &str, at: usize) -> Option<Loop> {
    let b = code.as_bytes();
    let mut j = at + "\\foreach".len();
    let blank = |j: &mut usize| {
        while *j < b.len() && (b[*j] as char).is_ascii_whitespace() {
            *j += 1;
        }
    };
    let mut vars = Vec::new();
    loop {
        blank(&mut j);
        if b.get(j) == Some(&b'/') {
            j += 1;
            continue;
        }
        if b.get(j) != Some(&b'\\') {
            break;
        }
        let e = command_name(b, j + 1);
        if e == j + 1 {
            return None;
        }
        vars.push(code[j..e].to_string());
        j = e;
    }
    if vars.is_empty() {
        return None;
    }
    let (mut count, mut first) = (None, 1);
    while b.get(j) == Some(&b'[') {
        let e = j + code[j..].find(']')?;
        let opts = &code[j + 1..e];
        if let Some(k) = opts.find("count=") {
            let rest = opts[k + 6..].trim_start();
            if rest.starts_with('\\') {
                let n = command_name(rest.as_bytes(), 1);
                count = Some(rest[..n].to_string());
                if let Some(v) = rest[n..].trim_start().strip_prefix("from") {
                    first = v.trim_start().split(|c: char| !(c.is_ascii_digit() || c == '-')).next()?.parse().unwrap_or(1);
                }
            }
        }
        j = e + 1;
        blank(&mut j);
    }
    if !code[j..].starts_with("in") {
        return None;
    }
    j += 2;
    blank(&mut j);
    if b.get(j) != Some(&b'{') {
        return None;
    }
    let close = find_outside_braces(code, j + 1, b'}')?;
    let values = foreach_values(&code[j + 1..close]);
    let mut s = close + 1;
    blank(&mut s);
    let braced = b.get(s) == Some(&b'{');
    let from = if braced { s + 1 } else { s };
    let stop = find_outside_braces(code, from, if braced { b'}' } else { b';' })?;
    let body = code[from..if braced { stop } else { stop + 1 }].to_string();
    Some(Loop { vars, count, first, values, body, end: stop + 1 })
}

/// Puts each variable's value in, a variable being a whole command name (`\a` does not touch `\alpha`).
fn put_values(body: &str, map: &[(&str, &str)]) -> String {
    let b = body.as_bytes();
    let (mut out, mut i, mut from) = (String::with_capacity(body.len()), 0, 0);
    while i < b.len() {
        if b[i] == b'\\' {
            let e = command_name(b, i + 1);
            if let Some((_, v)) = map.iter().find(|(n, _)| *n == &body[i..e]) {
                out.push_str(&body[from..i]);
                out.push_str(v);
                from = e;
            }
            i = e.max(i + 2);
        } else {
            i += 1;
        }
    }
    out.push_str(&body[from.min(body.len())..]);
    out
}

fn expand_loops(code: &str, room: &mut i64, depth: usize) -> String {
    if depth >= FOREACH_MAX_DEPTH {
        return code.to_string();
    }
    let (mut out, mut at) = (String::new(), 0);
    while let Some(k) = code[at..].find("\\foreach").map(|k| k + at) {
        let Some(l) = parse_foreach(code, k) else {
            let next = k + "\\foreach".len();
            out.push_str(&code[at..next]);
            at = next;
            continue;
        };
        out.push_str(&code[at..k]);
        at = l.end;
        let mut pass = l.first;
        for value in &l.values {
            let parts = if l.vars.len() > 1 { split_outside_braces(value, b'/') } else { vec![value.as_str()] };
            let pass_text = pass.to_string();
            pass += 1;
            let mut map: Vec<(&str, &str)> = l.vars.iter().enumerate().map(|(i, v)| (v.as_str(), parts[i.min(parts.len() - 1)])).collect();
            if let Some(c) = &l.count {
                map.push((c.as_str(), pass_text.as_str()));
            }
            let text = put_values(&l.body, &map);
            *room -= text.len() as i64;
            if *room < 0 {
                break;
            }
            out.push_str(&expand_loops(&text, room, depth + 1));
        }
    }
    out.push_str(&code[at.min(code.len())..]);
    out
}

/// A picture's `\foreach` loops written out, so that a label inside a loop counts once for each pass, as the picture
/// prints it. Outer loops go first, so an inner `in \row` finds the outer loop's value. The limits keep a huge loop from
/// freezing the count: past them the rest of the loop is not counted.
pub fn expand_foreach(code: &str) -> String {
    let mut room = FOREACH_MAX_BYTES;
    expand_loops(code, &mut room, 0)
}

/// What a TikZ picture prints, as text with maths kept: the labels of `tikz_labels`, loops written out, one per line.
pub fn picture_labels(code: &str) -> Vec<String> {
    let body = expand_foreach(code);
    let vars = foreach_vars(&body);
    tikz_labels(&body)
        .into_iter()
        .map(|(a, b)| {
            let mut label = body[a..b].to_string();
            for v in &vars {
                label = replace_command(&label, v, "0");
            }
            label
        })
        .collect()
}

/// The words of a block of code, as this crate counts words: runs between white space that hold a letter or a digit.
pub fn code_words(code: &str) -> usize {
    code.split_whitespace().filter(|w| w.chars().any(is_letter)).count()
}

/// The variables a TikZ picture's `\foreach` loops set (`\foreach \x/\name in …`, `[count=\k]`), with their backslash.
fn foreach_vars(body: &str) -> Vec<String> {
    let name_at = |k: usize| {
        let rest = body.get(k + 1..).unwrap_or("");
        let n = rest.bytes().take_while(u8::is_ascii_alphabetic).count();
        (body.as_bytes().get(k) == Some(&b'\\') && n > 0).then(|| body[k..k + 1 + n].to_string())
    };
    let mut vars = Vec::new();
    for (k, _) in body.match_indices("\\foreach") {
        let mut j = k + "\\foreach".len();
        loop {
            j += body[j..].bytes().take_while(|c| c.is_ascii_whitespace() || *c == b'/').count();
            match name_at(j) {
                Some(v) => {
                    j += v.len();
                    vars.push(v);
                }
                None => break,
            }
        }
    }
    for (k, _) in body.match_indices("count=") {
        let j = k + "count=".len();
        if let Some(v) = name_at(j + body[j..].bytes().take_while(u8::is_ascii_whitespace).count()) {
            vars.push(v);
        }
    }
    vars.sort();
    vars.dedup();
    vars
}

/// `text` with the command `name` (a backslash and letters) replaced where it is not the start of a longer name.
fn replace_command(text: &str, name: &str, with: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(k) = rest.find(name) {
        let after = &rest[k + name.len()..];
        out.push_str(&rest[..k]);
        if after.bytes().next().is_some_and(|c| c.is_ascii_alphabetic()) {
            out.push_str(name);
        } else {
            out.push_str(with);
        }
        rest = after;
    }
    out + rest
}

/// Where the environment `name` begun before `i` ends: the start of its \end and the index after it (an environment
/// of the same name inside it is passed over).
fn env_end(t: &str, i: usize, name: &str) -> (usize, usize) {
    let (open, close) = (format!("\\begin{{{name}}}"), format!("\\end{{{name}}}"));
    let (mut depth, mut k) = (1, i);
    loop {
        let (o, c) = (t[k..].find(&open).map(|x| x + k), t[k..].find(&close).map(|x| x + k));
        match (o, c) {
            (Some(o), Some(c)) if o < c => {
                depth += 1;
                k = o + open.len();
            }
            (_, Some(c)) => {
                depth -= 1;
                if depth == 0 {
                    return (c, c + close.len());
                }
                k = c + close.len();
            }
            _ => return (t.len(), t.len()),
        }
    }
}

/// The rows of an alignment: split at `\\` outside braces.
fn rows(body: &str) -> Vec<&str> {
    let b = body.as_bytes();
    let (mut out, mut depth, mut start, mut k) = (Vec::new(), 0i32, 0, 0);
    while k < b.len() {
        match b[k] {
            b'\\' if b.get(k + 1) == Some(&b'\\') && depth == 0 => {
                out.push(&body[start..k]);
                k += 2;
                start = k;
                continue;
            }
            b'\\' => k += 1,
            b'{' => depth += 1,
            b'}' => depth -= 1,
            _ => {}
        }
        k += 1;
    }
    out.push(&body[start.min(body.len())..]);
    out
}

/// The label of a paragraph comment, `% P3.2 | ...` (from its `%` to the end of its line): `P3.2`.
fn paragraph_label(line: &str) -> Option<String> {
    let rest = line.strip_prefix('%')?.trim_start().strip_prefix('P')?;
    let n: usize = rest.chars().take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '.').map(char::len_utf8).sum();
    (n > 0 && rest[n..].trim_start().starts_with('|')).then(|| format!("P{}", &rest[..n]))
}

#[cfg(test)]
mod tests;
