# view-marks

Marks over a note's text in Obsidian, for plugins written in Rust and compiled to WebAssembly: a plugin that colours
text by per-paragraph or per-sentence scores, or marks terms and passages, computes the scores; this crate splits the
note, says which paragraphs the views want scored, draws the marks in the editor (Live Preview and Source mode) and in
Reading view as the text comes into view, shows an optional pop-up over marks, and copies a selection with its marks
as Markdown markers. It came out of [Augmented View](https://github.com/Gameboard-dev/obsidian-augmented-view), which
uses it.

A plugin depends on it with `view-marks = { path = "../../view-marks" }`, beside `obsidian-kit` (whose `js` helpers it
uses and re-exports as `view_marks::js`).

## What is in it

Pure Rust, tested natively (`cargo test -p view-marks`), meant for a Web Worker where the plugin plans and scores:

| Module | What it does |
|---|---|
| `split` | A note's prose paragraphs (runs of lines between blank lines, each list item on its own; front matter, code, maths, comments, headings, tables, embeds, block ids, dividers and footnote definitions left out), each with UTF-16 offsets, lines, its word count and an xxh3 hash of its text, the key to keep its scores by. |
| `plan` | `Doc` (a note's text and paragraphs; paragraphs that just changed stay quiet for a while) and `View` (an editor's visible and rendered range, or a Reading view's lines, and when it last moved); `around(doc, view, now, context, timing)` lists the paragraphs a view wants, with their distance from the screen: none while it scrolls, those on screen once it rests, `context` more either side once it has rested longer. |
| `locate` | A passage marked in the source found in a Reading-view block's rendered text (spaces collapsed, case ignored if need be, a long passage by its first and last words), and overlapping classes turned into segments with a class mask (`segments`, `segments_with_strength`). |
| `highlights` | `write_range`: a selection's Markdown with each mark as a marker, `~={#hex, Name}text=~` or `<mark data-colour="#hex" data-category="Name">text</mark>`, never breaking the Markdown around it (front matter, code, maths, comments, list markers, headings, block ids, table pipes, links, footnotes, citations and HTML are respected; touching `~` and `=` are kept apart). |

Thin glue on Obsidian's main thread (CodeMirror and the DOM, reached by name through `obsidian-kit`):

| Module | What it does |
|---|---|
| `editor` | The CodeMirror view plugin: sends the note's text after 400 ms without typing and the visible range at most once a frame; draws the marks it gets back with one `Decoration.mark` per class (and per strength step, made on first use); maps them through edits until new ones come. |
| `reading` | Reading view's post-processor: blocks registered as Obsidian renders them, sent only when near the screen (an IntersectionObserver), wrapped in `<span>`s in animation frames with a 6 ms budget; elements held weakly and forgotten when Obsidian drops them. |
| `hover` | An optional pop-up over marks of chosen classes: its content is computed by the plugin on the first hover of a passage and kept by a hash of its class and text; shown as text, never HTML; placed inside the window. |
| `copy` | The editor's selection, the clipboard, an editor-menu item and an editor command, for "Copy with highlights". |
| `style` | The plugin's stylesheet in the document, hex colours and their dark-theme versions, rules scoped to a note's attribute, tints as a background or drawn over one, and the strength steps' rules. |

## How a plugin plugs in

**1. Its classes.** A `Config` names the base class every mark has, the attribute that scopes a note's editors and
Reading views, and the class of each mark class by index:

```rust
let cfg = view_marks::Config {
    base: "my".into(),                        // every mark: class "my"; the pop-up: "my-hover"; steps: "my-s1" … "my-s5"
    attr: "data-my-scheme".into(),            // on an editor's DOM and a Reading view's section: the note's scheme
    classes: vec!["my-low".into(), "my-mid".into(), "my-high".into(), "my-term".into()],
};
```

A mark (`Mark { from, to, class, strength }`, UTF-16 offsets in the note) is drawn with `"{base} {classes[class]}"`,
and with a strength step 1 to 5, also `"{base}-s{n}"`. Reading-view segments (`Seg { from, to, mask, strength }`)
carry a mask of classes (so up to 32 classes there) and one strength step for the segment.

**2. Its state.** The plugin keeps a `Marks` in its own state (a thread-local) and implements `Host`:

```rust
impl view_marks::Host for App {
    fn with<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> { with(f) }            // the plugin's thread-local
    fn marks(&mut self) -> &mut view_marks::Marks { &mut self.marks }
    fn scope(&self, path: &str) -> Option<String> { self.scheme_of(path) }         // None: the note is not marked
    fn send_doc(&mut self, path: &str, text: String) -> u32 { self.to_worker_doc(path, text) } // returns a version
    fn ensure_doc(&mut self, path: &str) { self.read_file_once(path) }             // Reading view: the file's text
    fn send(&mut self, e: view_marks::Event) { self.to_worker(e) }                 // View, DropView, Block, DropBlock
}
```

In `onload`, synchronously:

```rust
let marks = Marks::new::<App>(obsidian, require("@codemirror/view"), require("@codemirror/state"), cfg);
// … once the App holds it:
plugin.registerEditorExtension(view_marks::editor::extension(app));
plugin.registerMarkdownPostProcessor((el, ctx) => view_marks::reading::process(app, el, ctx));   // via obsidian-kit's f2
view_marks::style::set_stylesheet("my-plugin-colours", &css);
```

and in `onunload`, `marks.clear()` and `style::remove_stylesheet(…)`. When what `scope` answers changes (the user
links a note to another scheme), call `editor::tag_editor` for each of `editor::editor_ids` and
`marks.reading.tag(attr, …)`.

**3. Its scorer.** The events go wherever the plugin scores (a worker, through `postMessage`). There, per note, keep a
`plan::Doc` (`set_text` on each `send_doc`), per view a `plan::View` (`View::new` on each `Event::View`), and the
scores by paragraph hash (`Para::id`):

```rust
for (dist, i) in plan::around(&doc, &view, now, context, &timing) {
    let p = &doc.paras[i];
    if !scores.contains_key(&p.id) && !in_flight.contains(&p.id) {
        wanted.push((dist, p.id, doc.para_text(p)));                 // nearest the screen first: sort by dist
    }
}
```

Score what is wanted (a model, a server, rules), then answer each editor with its marks over its rendered range
(`doc.paras_in(view.render.0, view.render.1)`, offsets `p.from16 + …`, sorted by `from`), which the main thread draws
with `editor::apply::<App>(view_id, doc.version, &marks)` (outside `Host::with`), and each Reading-view block
(`Event::Block`: find its paragraph by the section's lines and its place, or by `locate::plain_words`) with segments
from `locate::Norm::new(&block.text).find_all(passage, every)` and `locate::segments_with_strength`, drawn with
`reading::queue(app, block_id, segs)`. `View::rests` and `Doc::quiet` say when to look again.

A per-sentence score becomes a mark per sentence: its class (the category, or a band of the score) and, if the
score's size should show, a strength step from 1 to 5. `style::stepped` writes the steps' rules (the opacity rising
from a fifth to all of the class's), and `style::rule` and `style::paint` the class's own.

**4. Pop-ups (optional).** After `Marks::new`:

```rust
view_marks::hover::install(app, &plugin, view_marks::hover::HoverSpec {
    classes: vec![0, 1, 2],                                         // marks of these classes have a pop-up
    delay_ms: 350.0,
    payload: Rc::new(|class, text| Box::pin(async move {             // called once per passage, then cached
        Some(Payload { title: "Why".into(), body: explain(class, &text).await? })
    })),
});
```

The passage is the mark's own range in an editor (the narrowest pop-up mark under the pointer) and the run of touching
segments with that class in Reading view. Contents are kept by `hover::key(class, text)` (the latest 1,000; `None`
answers are kept too, so nothing is asked twice). Style `.{base}-hover` (`position: fixed`), `.{base}-hover-title` and
`.{base}-hover-body` in the plugin's styles.css.

**5. Copy with highlights.** Where the marks are (the worker), `highlights::write_range(text, marks, colours, format,
from, to)` with `Mark { from, to, colour, phrase }` (a phrase is drawn inside its sentence) and `Named { colour,
name }` per colour; on the main thread, `copy::selection(editor)` for the text and offsets, `copy::to_clipboard`,
`copy::menu_item` in the `editor-menu` event and `copy::editor_command` for the keyboard.

## Performance

The editor never scans the note: marks are mapped through edits by CodeMirror, the text goes out only after typing
pauses, the visible range once a frame, and only the rendered range's marks come back. Reading view sends only blocks
near the screen and changes the DOM within a time budget per frame. `plan::around` looks only at the paragraphs within
reach of the screen (binary search, then the range), and `split` is one pass with no regular expressions. Augmented
View's measurements are in its `docs/performance.md`.

## Licence

MIT, as the workspace (`../LICENSE`).
