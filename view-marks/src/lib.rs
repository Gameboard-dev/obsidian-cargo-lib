//! Marks over a note's text in Obsidian, for plugins in Rust (WebAssembly): a plugin that colours text by
//! per-paragraph or per-sentence scores, or marks terms and passages, gives the scores; this crate does the rest.
//!
//! In a worker (pure Rust, tested natively):
//! - [`split`]: a note's prose paragraphs, with UTF-16 offsets, lines and a hash of each one's text;
//! - [`plan`]: which paragraphs the views want scored, nearest the screen first, once the view rests;
//! - [`locate`]: a source passage found in a Reading-view block's rendered text, and overlapping classes as segments;
//! - [`highlights`]: a selection's Markdown with its marks as markers, never breaking the Markdown around them.
//!
//! On Obsidian's main thread (thin glue over CodeMirror and the DOM):
//! - [`editor`]: the CodeMirror view plugin (Live Preview and Source mode): sends the text after typing pauses and the
//!   visible range once a frame, draws the marks it gets back, maps them through edits;
//! - [`reading`]: Reading view: blocks near the screen sent, segments wrapped in spans in frames with a time budget;
//! - [`hover`]: an optional pop-up over marks of chosen classes, its content computed on first hover and cached;
//! - [`copy`]: the selection, the clipboard, an editor-menu item and a command for "Copy with highlights";
//! - [`style`]: the plugin's stylesheet in the document, colour helpers and the strength steps' rules.
//!
//! A plugin implements [`Host`] (its state, what it sends where) and keeps a [`Marks`]. See README.md.

pub use obsidian_kit::js;

pub mod copy;
pub mod editor;
pub mod highlights;
pub mod hover;
pub mod locate;
pub mod plan;
pub mod reading;
pub mod split;
pub mod style;

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;

use js::{call, get, obj};

/// A mark in an editor: [from, to) in the note's UTF-16 offsets, the index of its class in [`Config::classes`], and a
/// strength step (0: none; 1 to 5, drawn with the extra class `{base}-s{n}`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mark {
    pub from: u32,
    pub to: u32,
    pub class: u8,
    pub strength: u8,
}

impl Mark {
    /// A mark with no strength step.
    pub fn new(from: u32, to: u32, class: u8) -> Mark {
        Mark { from, to, class, strength: 0 }
    }
}

/// A segment of a Reading-view block: [from, to) in the block's text (UTF-16), the classes over it (bit `i`: class `i`
/// of [`Config::classes`], so classes 0 to 31), and a strength step (0 to 5) for all of it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Seg {
    pub from: u32,
    pub to: u32,
    pub mask: u32,
    pub strength: u8,
}

impl From<(u32, u32, u32)> for Seg {
    fn from((from, to, mask): (u32, u32, u32)) -> Seg {
        Seg { from, to, mask, strength: 0 }
    }
}

/// The highest strength step.
pub const MAX_STRENGTH: u8 = 5;

/// What the main thread tells the host's planner (typically a worker): a view's state, a Reading-view block.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Event {
    /// An editor (`editor`: visible and rendered ranges in UTF-16 offsets) or a Reading view (visible lines; its id is
    /// [`reading::view_id`] of the note): answer an editor with [`editor::apply`].
    View { id: u32, path: String, editor: bool, vis: (u32, u32), render: (u32, u32) },
    DropView(u32),
    /// A rendered Reading-view block near the screen: its section's source lines (if Obsidian knows them), its place
    /// among the section's blocks, its text. Answer with [`reading::queue`].
    Block { id: u32, path: String, lines: Option<(u32, u32)>, idx: u32, n: u32, text: String },
    DropBlock(u32),
}

/// The plugin, as this crate needs it. Its state lives in a thread-local the plugin owns; every callback from
/// JavaScript reaches it through [`Host::with`].
pub trait Host: Sized + 'static {
    /// Run `f` on the plugin's state; None when it is gone or already borrowed (a callback during a call into
    /// JavaScript that calls back synchronously).
    fn with<R>(f: impl FnOnce(&mut Self) -> R) -> Option<R>;
    /// The crate's state, kept in the plugin's.
    fn marks(&mut self) -> &mut Marks;
    /// The value of [`Config::attr`] for a note's editors and Reading views (the plugin's stylesheet is scoped to it),
    /// or None: the note is not marked, and nothing of it is sent.
    fn scope(&self, path: &str) -> Option<String>;
    /// An editor's text (whole, after typing pauses): pass it on with a new version, and return the version, which
    /// comes back with the marks made for it ([`editor::apply`]).
    fn send_doc(&mut self, path: &str, text: String) -> u32;
    /// A Reading view shows a note: have its text (from the file) sent once, and again when the file changes.
    fn ensure_doc(&mut self, path: &str);
    /// Pass an event on.
    fn send(&mut self, event: Event);
}

/// The classes the marks are drawn with.
pub struct Config {
    /// The class every mark's element has (`av`, say): `span.{base}` is what Reading view unwraps, and the pop-up and
    /// strength classes are named after it (`{base}-hover`, `{base}-s1` … `{base}-s5`).
    pub base: String,
    /// The attribute on an editor's DOM and a Reading view's section that holds [`Host::scope`] (`data-…-code`).
    pub attr: String,
    /// The class of each mark class, by index ([`Mark::class`], [`Seg::mask`]'s bits).
    pub classes: Vec<String>,
}

/// The crate's state on the main thread: the editors, Reading view, the decorations, the pop-up.
pub struct Marks {
    pub(crate) cfg: Config,
    pub(crate) obs: JsValue,
    pub(crate) cmv: JsValue,
    pub(crate) cms: JsValue,
    /// `Decoration.mark` for each class, and for each class and strength step as first used.
    decos: Vec<JsValue>,
    /// By `class * MAX_STRENGTH + step - 1`; empty until a step is first drawn.
    stepped: Vec<JsValue>,
    pub(crate) refresh: JsValue,
    /// The view plugin ([`editor::extension`]), for the pop-up's look-ups.
    pub(crate) plugin: JsValue,
    pub(crate) editors: HashMap<u32, editor::Ed>,
    pub(crate) next_id: u32,
    pub reading: reading::Reading,
    pub(crate) hover: Option<hover::Hover>,
}

impl Marks {
    /// `obs`: the `obsidian` module; `cmv`, `cms`: `@codemirror/view` and `@codemirror/state` (from Obsidian's
    /// `require`). Make it synchronously in the plugin's `onload`, then register [`editor::extension`] and
    /// [`reading::process`].
    pub fn new<H: Host>(obs: JsValue, cmv: JsValue, cms: JsValue, cfg: Config) -> Marks {
        let decoration = get(&cmv, "Decoration");
        let decos = (0..cfg.classes.len()).map(|k| call!(decoration, mark, obj! { class: format!("{} {}", cfg.base, cfg.classes[k]) })).collect();
        let refresh = call!(get(&cms, "StateEffect"), define);
        Marks { cfg, obs, cmv, cms, decos, stepped: Vec::new(), refresh, plugin: JsValue::UNDEFINED, editors: HashMap::new(), next_id: 1, reading: reading::Reading::new::<H>(), hover: None }
    }

    /// The decoration of a class at a strength step (made once); undefined for a class not in [`Config::classes`].
    fn deco(&mut self, class: u8, strength: u8) -> JsValue {
        let Some(name) = self.cfg.classes.get(class as usize) else { return JsValue::UNDEFINED };
        if strength == 0 {
            return self.decos[class as usize].clone();
        }
        let strength = strength.min(MAX_STRENGTH);
        let i = class as usize * MAX_STRENGTH as usize + strength as usize - 1;
        if self.stepped.is_empty() {
            self.stepped = vec![JsValue::UNDEFINED; self.cfg.classes.len() * MAX_STRENGTH as usize];
        }
        if self.stepped[i].is_undefined() {
            let base = &self.cfg.base;
            self.stepped[i] = call!(get(&self.cmv, "Decoration"), mark, obj! { class: format!("{base} {name} {base}-s{strength}") });
        }
        self.stepped[i].clone()
    }

    /// A Reading-view wrapper's classes: the base, each class in the mask (lowest bit first), the strength step's.
    fn span_class(&self, mask: u32, strength: u8) -> String {
        span_class(&self.cfg, mask, strength)
    }

    /// Undo every Reading-view wrapping and remove the pop-up (the plugin's `onunload`).
    pub fn clear(&mut self) {
        self.reading.clear(&self.cfg.base);
        if let Some(h) = self.hover.take() {
            h.remove();
        }
    }

    pub fn config(&self) -> &Config {
        &self.cfg
    }
}

fn span_class(cfg: &Config, mask: u32, strength: u8) -> String {
    let mut s = cfg.base.clone();
    for (k, name) in cfg.classes.iter().enumerate().take(32) {
        if mask & (1 << k) != 0 {
            s.push(' ');
            s.push_str(name);
        }
    }
    if strength > 0 {
        s += &format!(" {}-s{}", cfg.base, strength.min(MAX_STRENGTH));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wrapper_takes_the_base_its_classes_and_its_step() {
        let cfg = Config { base: "vm".into(), attr: "data-vm".into(), classes: vec!["a".into(), "b".into(), "c".into()] };
        assert_eq!(span_class(&cfg, 0b101, 0), "vm a c");
        assert_eq!(span_class(&cfg, 0b010, 3), "vm b vm-s3");
        assert_eq!(span_class(&cfg, 0b1000, 9), "vm vm-s5", "a bit with no class adds nothing; steps stop at 5");
    }
}
