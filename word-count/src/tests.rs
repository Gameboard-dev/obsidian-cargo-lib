//! Each rule on a line or two. The counts of whole documents against the Lua counter's are tests/wordcount.rs.

use super::*;

const BIB: &str = r"
@article{one, author = {Smith, John}, title = {One}, year = {2020}}
@article{two, author = {Jones, Amy and Brown, Ben}, title = {Two}}
@article{three, author = {Alpha, Ann and Beta, Bob and Gamma, Cy}, title = {Three}}
@report{corp, author = {{Information Commissioner's Office}}, title = {Guidance}}
@article{prefix, author = {van der Berg, Jan}, title = {Prefix}}
@book{edited, editor = {Editor, Edith and Other, Oscar}, title = {Edited}}
@misc{none, title = {A Work without Author}}
@article{others, author = {Zhang, Wei and others}, title = {Others}}
@article{plain, author = {Jan van der Berg and Maria de la Torre}, title = {First names first}}
";

const GLOSSARY: &str = r"\term{FNRATE}{FNRATE}{Attack Presentation Classification Error Rate}{The share of attacks accepted.}
\term[known good presentation]{knowngood}{Known Good Presentation}{}{A presentation made in the intended manner.}
\term{W1}{$W_1$}{Wasserstein-1 Distance}{A distance.}
";

fn count_with(body: &str, rules: &Rules) -> Counts {
    let names = Names::from_bib(BIB);
    let glossary = Glossary::from_tex(GLOSSARY, "term");
    count_chapter(&format!("\\startwordcount{{k}}\n{body}\n\\stopwordcount\n"), &Context { rules, names: &names, glossary: &glossary, cites: true })
}

/// The count of a passage in a counted section, with the built-in rules.
fn n(body: &str) -> usize {
    count_with(body, &Rules::default()).sections[0].1
}

fn warnings(body: &str) -> Vec<String> {
    count_with(body, &Rules::default()).warnings.into_iter().map(|w| w.1).collect()
}

/// Every word counted in a chapter, with its section's key: to compare with the Lua counter's list.
fn trace(tex: &str, cx: &Context) -> Vec<(String, String)> {
    let tex = tex.replace("\r\n", "\n");
    let mut c = counter(cx, &tex);
    c.run(Src { text: &tex, base: 0, at: None, from: "" });
    c.m.trace.iter().map(|(k, w)| (c.keys[*k].clone(), w.clone())).collect()
}

/// WORDCOUNT_TRACE=<chapter.tex>: writes its words (key, tab, word) beside it as <chapter>.words.txt, reading the
/// .bib (My Library.bib) and glossary.tex beside it; to compare with the Lua counter's words when a count differs.
#[test]
fn trace_file() {
    let Ok(path) = std::env::var("WORDCOUNT_TRACE") else { return };
    let p = std::path::Path::new(&path);
    let dir = p.parent().unwrap();
    let names = Names::from_bib(&std::fs::read_to_string(dir.join("My Library.bib")).unwrap_or_default());
    let glossary = Glossary::from_tex(&std::fs::read_to_string(dir.join("glossary.tex")).unwrap_or_default(), "term");
    let rules = Rules::default();
    let words = trace(&std::fs::read_to_string(p).unwrap(), &Context { rules: &rules, names: &names, glossary: &glossary, cites: true });
    let out: String = words.iter().map(|(k, w)| format!("{k}\t{w}\n")).collect();
    std::fs::write(p.with_extension("words.txt"), out).unwrap();
}

#[test]
fn words_and_spaces() {
    assert_eq!(n("A plain sentence of six words."), 6);
    assert_eq!(n("one-to-one, state-of-the-art"), 2, "a hyphen joins");
    assert_eq!(n("pages 4--5, a---b, a -- b, a - b"), 9,"-- and --- are dashes, which end a word; a dash or hyphen alone is none");
    assert_eq!(n("an en dash – here, an em dash — there"), 8);
    assert_eq!(n("Mr.~Smith and 372\\,ms and 10\\,000 and 7\\,\\%"), 8, "a tie is a space; a thin space is not");
    assert_eq!(n("50\\% and ( ) and ``quoted'' and it's"), 6);
    assert_eq!(n("café naïve Łukasz a×b 5 × 6 x ≈ y ellipsis… on"), 12, "beyond ASCII, every character is a letter");
    assert_eq!(n("a\u{a0}b c"), 2, "a no-break space typed as such is a letter, not a space");
    assert_eq!(n("a line\nand the next"), 5);
    assert_eq!(n("first\n\nsecond"), 2);
    assert_eq!(n("joined%comment\nup"), 1, "a comment swallows its line end");
    assert_eq!(n("word % a comment of five words\nnext"), 2);
}

#[test]
fn counted_sections() {
    let r = Rules::default();
    let names = Names::default();
    let g = Glossary::default();
    let cx = Context { rules: &r, names: &names, glossary: &g, cites: true };
    let c = count_chapter("Before.\n\\startwordcount{a}\n\\section{Two Words}\n\\label{sec:a}\n\\printwordcount{a}\n\nThree more words.\n\\startwordcount{b}\nOne.\n\\stopwordcount\nAfter.\n", &cx);
    assert_eq!(c.sections, vec![("a".into(), 5), ("b".into(), 1)]);
    assert!(c.warnings.is_empty(), "{:?}", c.warnings);
    assert_eq!(n("{\\wordcountoff Not these words.} These."), 1, "the off macro holds for its group");
    assert_eq!(n("\\textbf{\\wordcountoff Not} but these"), 2);
}

#[test]
fn headings_and_lists() {
    assert_eq!(n("\\section{Text and Emphasis}\\label{x}"), 3);
    assert_eq!(n("\\subsection*{Unnumbered Heading}\n\\phantomsection\\addcontentsline{toc}{subsection}{Unnumbered Heading}"), 2);
    assert_eq!(n("\\paragraph{A Run-in Heading} Its text."), 5);
    assert_eq!(n("\\begin{enumerate}\n\\item First\n\\item Second\n\\end{enumerate}"), 4, "an enumerate's number is a word");
    assert_eq!(n("\\begin{itemize}\n\\item First\n\\item Second\n\\end{itemize}"), 2, "an itemize's bullet is not");
    assert_eq!(n("\\begin{description}\\item[Term one] text \\item no label\\end{description}"), 5);
    assert_eq!(n("\\begin{enumerate}\n  \\item[\\textbf{RQ1}] \\hypertarget{rq1}{}Does it hold?\n\\end{enumerate}"), 4);
}

#[test]
fn citations() {
    assert_eq!(n("A citation \\cite{one}. A page \\cite[p.~27]{one}. Keys \\cite{one, two}."), 11, "as printed: [1], [1, p. 27], [1], [2]");
    assert_eq!(n("A prefix \\cite[see][p.~2]{two}."), 6, "A prefix [see 2, p. 2]");
    assert_eq!(n("and \\cites[p.~3]{one}{two} and"), 6);
    assert_eq!(n("and \\cites{one}[see][p.~4]{two}{three}."), 7);
    assert_eq!(n("word\\cite{one}"), 2, "a citation is not part of the word before it");
    assert_eq!(n("\\citet{one} argues"), 3, "Smith [1] argues");
    assert_eq!(n("\\citet[p.~4]{two}"), 6, "Jones and Brown [2, p. 4]");
    assert_eq!(n("\\Citet{three}"), 4, "Alpha et al. [3]");
    assert_eq!(n("\\citet{corp}"), 4, "Information Commissioner's Office [4]");
    assert_eq!(n("\\citet{prefix} \\citet{plain}"), 2 + 4, "Berg [5]; Berg and Torre [6]: the prefix is not printed");
    assert_eq!(n("\\citet{edited}"), 4, "Editor and Other: the editors stand in for authors");
    assert_eq!(n("\\citet{others}"), 4, "Zhang et al.");
    assert_eq!(n("\\citet{none}"), 5, "a work without names prints its title");
    assert_eq!(n("\\citet{one,two}"), 7, "Smith [1] and Jones and Brown [2]");
    assert_eq!(n("\\citet{one,two,three}"), 11, "Smith [1], Jones and Brown [2], and Alpha et al. [3]");
    assert_eq!(n("\\citeauthor{two} \\citeauthor{none} \\citep{one} \\citeyear{one} \\nocite{two}"), 5, "Jones and Brown, nothing, [1], 2020, nothing");
    assert_eq!(n("\\textcites{one}{two}"), 7);
    assert_eq!(warnings("\\cite{one} \\citet{missing}"), ["no entry missing in the .bib: its names were counted as one word"]);
}

#[test]
fn glossary_and_references() {
    assert_eq!(n("\\gls{FNRATE}, \\glspl{FNRATE}, \\Gls{knowngood}, \\glspl{knowngood}"), 1 + 1 + 3 + 3);
    assert_eq!(n("\\gls{W1} and \\glspl{W1}"), 2, "maths counts nothing, the plural's s is a word: and, s");
    assert_eq!(n("\\glslink{FNRATE}{the attack error}"), 3);
    assert_eq!(n("Section~\\ref{sec:a}, (Section~\\ref{sec:a}) and \\eqref{eq:a}"), 6);
    assert_eq!(n("A\\ref{x}"), 1, "a reference runs into the characters around it");
    assert_eq!(n("\\rqref{1} and \\hypref{2a}"), 3);
    assert_eq!(warnings("\\gls{nope}"), ["no glossary entry nope: counted as one word"]);
}

#[test]
fn footnotes_and_styles() {
    assert_eq!(n("a footnote\\footnote{Of four more words.} and on"), 8);
    assert_eq!(n("foot\\footnote{x}note"), 2, "the mark is not a word, and the word around it runs on");
    assert_eq!(n("\\emph{one} \\textit{two} \\textbf{three} \\texttt{code\\_name--x} \\textcolor{red}{red words}"), 7);
    assert_eq!(n("a \\href{https://example.org/a-b_c}{link text} and \\url{https://example.org}"), 4);
    assert_eq!(n("see\\url{x}now"), 2, "\\url is maths: the word before it ends");
    assert_eq!(n("a\\mbox{b c}d"), 2);
    assert_eq!(n("caf\\'e na\\\"{\\i}ve Ma\\ss"), 3);
    assert_eq!(n("a\\quad b\\qquad c\\hspace{1em}d\\hfill e"), 5);
    assert_eq!(n("a\\textendash b\\textemdash c"), 3);
}

#[test]
fn maths() {
    assert_eq!(n("Inline $x^2 + y$ maths, an $n$-fold rise, a 10$\\times$ gain, $5$\\% and \\(a\\)."), 9);
    assert_eq!(n("x$y$z"), 2, "maths ends the word before it");
    assert_eq!(n("\\[ a = b \\]\nThen $$ c $$ more"), 3, "a $$ block is one word");
    assert_eq!(n("\\begin{equation}E = mc^2\\label{eq:a}\\end{equation}"), 1, "an equation is one word");
    assert_eq!(n("\\begin{align}\na &= b \\\\\nc &= d \\nonumber \\\\\ne &= f \\tag{T}\n\\end{align}"), 1, "so is an align, however many rows it has");
    assert_eq!(n("\\begin{align*}a \\\\ b\\end{align*} \\begin{gather}a \\\\ b\\end{gather} \\begin{multline}a \\\\ b\\end{multline}"), 3);
}

#[test]
fn floats_and_code() {
    // captions with their labels, table cells, code and the text a picture prints all count
    assert_eq!(n("\\begin{table}[htbp]\n\\caption{Now counted}\n\\begin{tabular}{ll}a & b\\end{tabular}\n\\end{table}\nAfter."), 7, "Figure 3: Now counted a b After.");
    assert_eq!(n("\\begin{figure}\\begin{tikzpicture}[node distance=1cm]\\node {x};\\end{tikzpicture}\\caption{Yes}\\end{figure}"), 4, "x, Figure 3: Yes");
    assert_eq!(n("\\begin{handtyped}[htbp]\n\\handtypedcode{bench}%code-words 3\n\\caption{Yes}\n\\end{handtyped}"), 3 + 2 + 1, "the code's words, the label, the caption");
    assert_eq!(n("\\begin{verbatim}\nverbatim code block \\words\n\\end{verbatim}"), 4, "code counts as written");
}

#[test]
fn unknown_commands_are_reported() {
    let c = count_with("Some \\foo{bar} and \\foo{baz}.\n\\begin{myenv}x\\end{myenv}", &Rules::default());
    assert_eq!(c.sections[0].1, 5, "the arguments are counted as text: Some bar and baz. x");
    assert_eq!(
        c.warnings,
        [
            (2, "\\foo has no word-count rule, so what it holds was counted as text (2 times from here): add a rule for \\foo to wordcountRules in the plugin's data.json".to_string()),
            (3, "the environment myenv has no word-count rule, so what it holds was counted as text: add a rule for the environment myenv to wordcountRules in the plugin's data.json".to_string()),
        ]
    );
    assert!(warnings("{\\wordcountoff \\foo{x}}").is_empty(), "only in counted text");
    assert!(warnings("\\begin{tikzpicture}\\foo\\end{tikzpicture}").is_empty(), "nor in a picture's drawing commands");
    let names = Names::default();
    let glossary = Glossary::from_tex("\\term{G}{\\weird{x}}{}{}", "term");
    let r = Rules::default();
    let c = count_chapter("\\startwordcount{k}\n\\gls{G}\n", &Context { rules: &r, names: &names, glossary: &glossary, cites: true });
    assert_eq!(c.warnings[0], (2, "\\weird (the glossary entry G) has no word-count rule, so what it holds was counted as text: add a rule for \\weird to wordcountRules in the plugin's data.json".to_string()));
}

#[test]
fn user_rules() {
    let m = Macros::default();
    let (skip_emph, errors) = Rules::new("% emphasis is left out\n\\emph m skip\n", &m);
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(n("\\emph{three more words} here"), 4);
    assert_eq!(count_with("\\emph{three more words} here", &skip_emph).sections[0].1, 1, "a user rule changes a count");
    let (bullets_off, _) = Rules::new("itemize list-plain\n", &m);
    assert_eq!(count_with("\\begin{itemize}\n\\item First\n\\end{itemize}", &bullets_off).sections[0].1, 1, "a user rule overrides a default");
    let (mine, _) = Rules::new("\\mycite oom one\n\\keyword m text\n\\todo om skip\nmyenv skip\n\\pair mm text=1", &m);
    let c = count_with("\\mycite[p.~2]{x} \\keyword{a b} \\todo[inline]{not this} \\begin{myenv}no\\end{myenv} \\pair{one}{two}", &mine);
    assert_eq!(c.sections[0].1, 1 + 2 + 1);
    assert!(c.warnings.is_empty(), "{:?}", c.warnings);
    // a comment after a rule; \% is a name, not a comment
    let (commented, errors) = Rules::new("\\emph m skip % emphasis is left out\n\\% skip\n", &m);
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(count_with("\\emph{three more words} here", &commented).sections[0].1, 1);
}

#[test]
fn malformed_rules_are_reported() {
    let (_, errors) = Rules::new("\\good m text\n\\bad m nonsense\nenv\n\\x q text\n\\y m text=3\n\\z skip=1\n\\ m text\n", &Macros::default());
    assert_eq!(errors.len(), 6, "{errors:?}");
    assert!(errors[0].starts_with("wordcountRules, line 2: nonsense is not a command behaviour (text, heading,"), "{}", errors[0]);
    assert!(errors[1].starts_with("wordcountRules, line 3: a rule is a name"), "{}", errors[1]);
    assert!(errors[2].starts_with("wordcountRules, line 4: q is not an argument list"), "{}", errors[2]);
    assert!(errors[3].starts_with("wordcountRules, line 5: \\y has no argument 3"), "{}", errors[3]);
    assert!(errors[4].starts_with("wordcountRules, line 6: only text, heading, runin and footnote take =n"), "{}", errors[4]);
    assert!(errors[5].starts_with("wordcountRules, line 7:"), "{}", errors[5]);
}

#[test]
fn macro_names_are_settings() {
    let m = Macros { start: "countstart".into(), print: "countprint".into(), stop: "countstop".into(), off: "nocount".into() };
    let (r, errors) = Rules::new("", &m);
    assert!(errors.is_empty());
    let (names, g) = (Names::default(), Glossary::default());
    let c = count_chapter("\\countstart{s}\\countprint{s}\nOne {\\nocount two} three.\n\\countstop\nFour.", &Context { rules: &r, names: &names, glossary: &g, cites: true });
    assert_eq!(c.sections, vec![("s".into(), 2)]);
}

#[test]
fn bib_names() {
    assert_eq!(family("van der Berg, Jan"), "Berg");
    assert_eq!(family("Jan van der Berg"), "Berg");
    assert_eq!(family("de la Torre, Maria"), "Torre");
    assert_eq!(family("Le Roux, Paul"), "Le Roux");
    assert_eq!(family("{Information Commissioner's Office}"), "Information Commissioner's Office");
    assert_eq!(family("Mart{\\'\\i}nez-D{\\'\\i}az, Laura"), "Mart{\\'\\i}nez-D{\\'\\i}az");
    assert_eq!(family("John Smith"), "Smith");
    assert_eq!(cited_names("Smith, J. and Jones, A. and Brown, B."), "Smith et al.");
    assert_eq!(cited_names("Smith, J. and others"), "Smith et al.");
    let names = Names::from_bib(BIB);
    assert_eq!(names.get("two").unwrap().0, "Jones and Brown");
    assert_eq!(names.get("none").unwrap(), &(String::new(), "A Work without Author".to_string()));
}

#[test]
fn inline_maths_as_one_word() {
    let body = r"One $x^2$ two \(y\), three$z$ four. $$a = b$$";
    assert_eq!(n(body), 5, "off: inline maths counts nothing; the $$ block is one word");
    let mut r = Rules::default();
    r.inline_math_word = true;
    // each inline formula one word of its own, also when run into a word; the displayed block one more
    assert_eq!(count_with(body, &r).sections[0].1, 8);
}

fn paras(body: &str) -> Vec<(String, usize, bool)> {
    count_with(body, &Rules::default()).paras.into_iter().map(|p| (p.label, p.n, p.float)).collect()
}

#[test]
fn paragraphs_with_their_labels() {
    let body = "\\section{Two Words}\n\n% P1.1 | the first\nOne two three.\n\n% P1.2 | the second\nFour five\\footnote{Six seven.} eight.\n\nNine.";
    assert_eq!(
        paras(body),
        vec![("heading".into(), 2, false), ("P1.1".into(), 3, false), ("P1.2".into(), 5, false), ("-".into(), 1, false)],
        "a footnote joins the paragraph of its mark; a blank line ends a label"
    );
    let c = count_with(body, &Rules::default());
    assert_eq!(c.paras.iter().map(|p| p.n).sum::<usize>(), c.sections[0].1, "the paragraphs add up to the section");
    assert_eq!(c.paras[2].text, "Four five Six seven. eight.");
    // a display formula or a list inside a paragraph splits it; the label stays for its parts
    assert_eq!(paras("% P2 | x\nBefore\n\\[ a \\]\nafter."), vec![("P2".into(), 1, false), ("P2".into(), 1, false)]);
    // not a label: no number, no bar, or a comment that is not a paragraph's
    for c in ["% P | x", "% P3.2 the text", "% Note | x", "%P3"] {
        assert_eq!(paras(&format!("{c}\nWords here.")), vec![("-".into(), 2, false)], "{c}");
    }
    assert_eq!(paras("%P3|x\nWords."), vec![("P3".into(), 1, false)]);
}

#[test]
fn a_floats_caption_is_a_paragraph_of_its_own_and_the_count_does_not_change() {
    let float = "Before the figure\n\\begin{figure}[ht]\n\\centering\n\\includegraphics{a.png}\n\\caption{A small caption.}\n\\end{figure}\nand after it.";
    let c = count_with(float, &Rules::default());
    let got: Vec<(String, usize, bool)> = c.paras.iter().map(|p| (p.label.clone(), p.n, p.float)).collect();
    assert_eq!(got, vec![("-".into(), 3, false), ("-".into(), 5, true), ("-".into(), 3, false)]);
    assert_eq!(c.paras[1].text, "Figure 0: A small caption.");
    // the same words without the float boundaries: the count is the plugin's, recording paragraphs changes nothing
    assert_eq!(c.sections[0].1, 11);
    assert_eq!(n("Before the figure\n\\begin{center}\n\\caption{A small caption.}\n\\end{center}\nand after it."), 11);
}

#[test]
fn citations_switched_off() {
    let body = "As \\textcite{two} found \\parencite[p.~2]{one}, and \\citeauthor{three} too.";
    let names = Names::from_bib(BIB);
    let glossary = Glossary::from_tex(GLOSSARY, "term");
    let count = |cites: bool| {
        let cx = Context { rules: &Rules::default(), names: &names, glossary: &glossary, cites };
        count_chapter(&format!("\\startwordcount{{k}}\n{body}\n\\stopwordcount\n"), &cx)
    };
    let on = count(true);
    assert_eq!(on.sections[0].1, 14, "As Jones and Brown [0] found [0], and Alpha et al. too.");
    assert!(on.paras[0].text.contains("[0]"), "a citation shows as [0]: {}", on.paras[0].text);
    assert_eq!(count(false).sections[0].1, 4, "As found, and too.");
}

#[test]
fn glossaries_joined() {
    let mut g = Glossary::from_tex(GLOSSARY, "term");
    g.extend(&Glossary::from_tex("\\term{W1}{W1}{Wasserstein Distance}{}\n", "term"));
    g.insert("ACR", "ACR");
    let names = Names::default();
    let c = count_chapter("\\startwordcount{k}\n\\gls{W1} \\gls{ACR} \\gls{knowngood}\n", &Context { rules: &Rules::default(), names: &names, glossary: &g, cites: true });
    assert_eq!(c.sections[0].1, 1 + 1 + 3, "the later W1 replaces $W_1$ (which counts nothing); the inserted ACR; the first file's own");
    assert!(c.warnings.is_empty(), "{:?}", c.warnings);
}

#[test]
fn a_picture_counts_what_it_prints() {
    assert_eq!(n("\\begin{tikzpicture}\\node[draw] (a) at (0,0) {two words};\\draw (a) -- (b);\\end{tikzpicture}"), 2, "no drawing commands");
    assert_eq!(n("\\begin{tikzpicture}\\foreach \\x in {1,2,3} {\\node at (\\x,0) {item \\x};}\\end{tikzpicture}"), 6, "once per pass");
    assert_eq!(n("\\begin{tikzpicture}\\foreach \\x in {0,...,4} {\\node {\\x};}\\end{tikzpicture}"), 5);
    assert_eq!(n("\\begin{tikzpicture}\\foreach \\x in {0,2,...,10} {\\node {\\x};}\\end{tikzpicture}"), 6);
    assert_eq!(n("\\begin{tikzpicture}\\foreach \\a/\\b in {x/one,y/two} {\\node {\\b};}\\end{tikzpicture}"), 2);
    assert_eq!(n("\\begin{tikzpicture}\\foreach \\p/\\t in {0/First/{1,2},6/Second/{3,4}} {\\node {\\t};}\\end{tikzpicture}"), 2);
    let nested = "\\begin{tikzpicture}\\foreach \\row [count=\\f] in {{1,2,3},{4,5,6}}{\\node {filter \\f};\\foreach \\v in \\row {\\node {\\v};}}\\end{tikzpicture}";
    assert_eq!(n(nested), 10, "an inner loop over a value of the outer loop");
    assert_eq!(n("\\begin{tikzpicture}\\foreach \\x in \\mylist {\\node {word \\x};}\\end{tikzpicture}"), 2, "a list defined elsewhere: once");
    assert_eq!(n("\\begin{tikzpicture}\\foreach \\x in {a,b} \\node at (0,0) {\\x};\n\\node {tail};\\end{tikzpicture}"), 3, "a loop without braces");
    let mut r = Rules::default();
    r.inline_math_word = true;
    let c = count_with("\\begin{tikzpicture}\\node {range $[a,b]$};\\foreach \\v in {1,2} {\\node {$\\v$};}\\end{tikzpicture}", &r);
    assert_eq!(c.sections[0].1, 4, "maths in a label follows the inline maths rule");
    let huge = "\\begin{tikzpicture}\\foreach \\x in {1,...,100000000} {\\node {w};}\\end{tikzpicture}";
    assert!((1..=1000).contains(&n(huge)), "a huge loop stops at its limit");
}
