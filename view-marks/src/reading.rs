//! Reading view. Rendered blocks are registered as Obsidian renders them, but only those near the screen (an
//! IntersectionObserver, one screen either side) are sent to the host, and the DOM is changed in animation frames
//! with a time budget, so wrapping never holds up scrolling. Elements are held weakly and forgotten when Obsidian
//! discards them (FinalizationRegistry). The host maps each block to its source paragraph and returns segments with
//! the classes over them ([`crate::locate`]); this file only splits text nodes.
//!
//! References: Obsidian's post-processors (<https://docs.obsidian.md/Plugins/Editor/Markdown+post+processing>),
//! <https://developer.mozilla.org/en-US/docs/Web/API/IntersectionObserver>, <https://developer.mozilla.org/en-US/docs/Web/API/Window/requestAnimationFrame>,
//! <https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/WeakRef>, <https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/FinalizationRegistry>.

use std::collections::{HashMap, HashSet, VecDeque};

use js_sys::{Array, JsString};
use wasm_bindgen::prelude::*;

use crate::js::{self, call, f1, get, new, obj, set, str};
use crate::{Event, Host, Seg};

const BLOCKS: &str = "p, li";
const SKIP_BLOCK: &str = "pre, table, .frontmatter-container, .internal-embed";
const SKIP_TEXT: &str = "code, .math, mjx-container, .internal-embed, sup.footnote-ref"; // text in a block that is not prose
const FRAME_BUDGET: f64 = 6.0; // ms of DOM work per animation frame
/// The property (not an attribute: no DOM change) that holds a block's id on its element.
pub(crate) const BLOCK_ID: &str = "__vmb";
/// On a wrapper span, when pop-ups are on: its segment's place in the block's segments.
pub(crate) const SEG_INDEX: &str = "__vms";

pub(crate) struct Rec {
    el: JsValue,      // WeakRef
    section: JsValue, // WeakRef
    ctx: JsValue,
    path: String,
    idx: u32,
    n: u32,
    pub(crate) text: Option<String>, // as sent; None until it first comes near the screen
    lines: Option<(u32, u32)>,
    /// The segments drawn, kept only when pop-ups are on (hover.rs).
    pub(crate) segs: Vec<Seg>,
}

pub struct Reading {
    pub(crate) recs: HashMap<u32, Rec>,
    near: HashMap<String, HashSet<u32>>,
    queue: VecDeque<(u32, Vec<Seg>)>,
    frame: bool,
    views_due: bool,
    next: u32,
    observer: JsValue,
    registry: JsValue,
}

fn weak(v: &JsValue) -> JsValue {
    new(&get(&js::global(), "WeakRef"), std::slice::from_ref(v))
}

impl Reading {
    pub(crate) fn new<H: Host>() -> Reading {
        let g = js::global();
        let observer = new(&get(&g, "IntersectionObserver"), &[
            f1(|entries| {
                H::with(|h| intersect(h, &entries));
                JsValue::UNDEFINED
            }),
            obj! { rootMargin: "100% 0px" },
        ]);
        let registry = new(&get(&g, "FinalizationRegistry"), &[f1(|id| {
            H::with(|h| forget(h, id.as_f64().unwrap_or(0.0) as u32));
            JsValue::UNDEFINED
        })]);
        Reading { recs: HashMap::new(), near: HashMap::new(), queue: VecDeque::new(), frame: false, views_due: false, next: 1, observer, registry }
    }

    /// The notes whose Reading-view blocks are registered.
    pub fn paths(&self) -> Vec<String> {
        let mut p: Vec<String> = self.recs.values().map(|r| r.path.clone()).collect();
        p.sort();
        p.dedup();
        p
    }

    pub fn shows(&self, path: &str) -> bool {
        self.recs.values().any(|r| r.path == path)
    }

    /// Every registered block's section takes its note's scope (`attr` = [`crate::Config::attr`]), or none.
    pub fn tag(&self, attr: &str, code: impl Fn(&str) -> Option<String>) {
        for r in self.recs.values() {
            let section = call!(r.section, deref);
            if section.is_undefined() {
                continue;
            }
            match code(&r.path) {
                Some(c) => _ = call!(section, setAttribute, attr, c),
                None => _ = call!(section, removeAttribute, attr),
            }
        }
    }

    pub(crate) fn clear(&mut self, base: &str) {
        call!(self.observer, disconnect);
        for r in self.recs.values() {
            let el = call!(r.el, deref);
            if !el.is_undefined() {
                unwrap(&el, base);
            }
        }
        self.recs.clear();
        self.queue.clear();
    }
}

/// The host's segments for a block (its [`Event::Block`] id): drawn in the next animation frames.
pub fn queue<H: Host>(h: &mut H, block: u32, segs: Vec<Seg>) {
    let r = &mut h.marks().reading;
    r.queue.retain(|(b, _)| *b != block);
    r.queue.push_back((block, segs));
    if !r.frame {
        r.frame = true;
        js::frame(|| _ = H::with(|h| drain(h)));
    }
}

/// Obsidian's Markdown post-processor (<https://docs.obsidian.md/Reference/TypeScript+API/Plugin/registerMarkdownPostProcessor>):
/// register it as `(el, ctx) => process(host, el, ctx)`.
pub fn process<H: Host>(h: &mut H, section: JsValue, ctx: JsValue) {
    let found = if call!(section, matches, BLOCKS).is_truthy() { Array::of1(&section) } else { Array::from(&call!(section, querySelectorAll, BLOCKS)) };
    let blocks: Vec<JsValue> = found
        .iter()
        .filter(|b| call!(b, closest, SKIP_BLOCK).is_null() && !(str(&get(b, "tagName")) == "LI" && !call!(b, querySelector, "ul, ol, p").is_null()))
        .collect(); // a list item with nested blocks: its <p>s are taken instead
    if blocks.is_empty() {
        return;
    }
    let path = str(&get(&ctx, "sourcePath"));
    let Some(code) = h.scope(&path) else { return }; // not marked: nothing is sent
    call!(section, setAttribute, h.marks().cfg.attr.clone(), code);
    h.ensure_doc(&path);
    let r = &mut h.marks().reading;
    for (idx, b) in blocks.iter().enumerate() {
        let id = r.next;
        r.next += 1;
        set(b, BLOCK_ID, id);
        r.recs.insert(id, Rec { el: weak(b), section: weak(&section), ctx: ctx.clone(), path: path.clone(), idx: idx as u32, n: blocks.len() as u32, text: None, lines: None, segs: Vec::new() });
        call!(r.registry, register, b.clone(), id);
        call!(r.observer, observe, b.clone());
    }
}

fn intersect<H: Host>(h: &mut H, entries: &JsValue) {
    for e in Array::from(entries).iter() {
        let el = get(&e, "target");
        let id = get(&el, BLOCK_ID).as_f64().unwrap_or(0.0) as u32;
        let r = &mut h.marks().reading;
        let Some(path) = r.recs.get(&id).map(|r| r.path.clone()) else { continue };
        let near = r.near.entry(path).or_default();
        if get(&e, "isIntersecting").is_truthy() {
            near.insert(id);
            if r.recs[&id].text.is_none() {
                register_block(h, id, &el);
            }
        } else {
            near.remove(&id);
        }
    }
    let r = &mut h.marks().reading;
    if !r.views_due {
        r.views_due = true;
        js::timeout(100.0, || _ = H::with(|h| send_views(h)));
    }
}

/// Tell the host about a block once it comes near the screen (the DOM is settled by then). Its source lines:
/// <https://docs.obsidian.md/Reference/TypeScript+API/MarkdownPostProcessorContext/getSectionInfo> (may be null).
fn register_block<H: Host>(h: &mut H, id: u32, el: &JsValue) {
    let Some(r) = h.marks().reading.recs.get_mut(&id) else { return };
    let section = call!(r.section, deref);
    let info = if section.is_undefined() { JsValue::NULL } else { call!(r.ctx, getSectionInfo, section) };
    let line = |k: &str| get(&info, k).as_f64().map(|v| v as u32);
    r.lines = if info.is_null() || info.is_undefined() { None } else { line("lineStart").zip(line("lineEnd")) };
    let text: String = text_nodes(el).iter().map(|n| str(&get(n, "data"))).collect();
    r.text = Some(text.clone());
    let m = Event::Block { id, path: r.path.clone(), lines: r.lines, idx: r.idx, n: r.n, text };
    h.send(m);
}

/// The lines near the screen in each note's Reading view, for the host's plan.
fn send_views<H: Host>(h: &mut H) {
    h.marks().reading.views_due = false;
    let mut msgs = Vec::new();
    let paths: Vec<String> = h.marks().reading.near.keys().cloned().collect();
    let coloured: HashSet<String> = paths.into_iter().filter(|p| h.scope(p).is_some()).collect();
    let r = &mut h.marks().reading;
    let recs = &r.recs;
    r.near.retain(|path, set| {
        let id = view_id(path);
        if !coloured.contains(path) {
            return true; // not marked: not sent (retag dropped its view)
        }
        let lines = set.iter().filter_map(|b| recs.get(b)?.lines);
        match lines.fold(None, |acc: Option<(u32, u32)>, (a, b)| Some(acc.map_or((a, b), |(x, y)| (x.min(a), y.max(b))))) {
            Some(vis) => msgs.push(Event::View { id, path: path.clone(), editor: false, vis, render: (0, 0) }),
            None if set.is_empty() => {
                msgs.push(Event::DropView(id));
                return false;
            }
            None => {}
        }
        true
    });
    msgs.into_iter().for_each(|m| h.send(m));
}

fn drain<H: Host>(h: &mut H) {
    h.marks().reading.frame = false;
    let t0 = js::now();
    let (base, hover) = (h.marks().cfg.base.clone(), h.marks().hover.is_some());
    while let Some((id, segs)) = h.marks().reading.queue.pop_front() {
        let m = h.marks();
        let Some(r) = m.reading.recs.get(&id) else { continue };
        let el = call!(r.el, deref);
        if !el.is_undefined() && get(&el, "isConnected").is_truthy() {
            unwrap(&el, &base);
            let nodes = text_nodes(&el);
            if Some(nodes.iter().map(|n| str(&get(n, "data"))).collect::<String>()) != r.text {
                register_block(h, id, &el); // re-rendered with other text: map again
            } else {
                apply_segments(&nodes, &segs, m, hover);
                if hover {
                    if let Some(r) = m.reading.recs.get_mut(&id) {
                        r.segs = segs;
                    }
                }
            }
        }
        if js::now() - t0 > FRAME_BUDGET {
            break; // the rest next frame
        }
    }
    let r = &mut h.marks().reading;
    if !r.queue.is_empty() && !r.frame {
        r.frame = true;
        js::frame(|| _ = H::with(|h| drain(h)));
    }
}

fn forget<H: Host>(h: &mut H, id: u32) {
    let r = &mut h.marks().reading;
    if let Some(rec) = r.recs.remove(&id) {
        r.near.get_mut(&rec.path).map(|s| s.remove(&id));
    }
    h.send(Event::DropBlock(id));
}

/// A Reading view's id for the host, kept clear of the editors' small ids.
pub fn view_id(path: &str) -> u32 {
    (xxhash_rust::xxh3::xxh3_64(path.as_bytes()) as u32 >> 1) | 0x4000_0000
}

fn text_nodes(root: &JsValue) -> Vec<JsValue> {
    fn walk(el: &JsValue, out: &mut Vec<JsValue>) {
        for c in Array::from(&get(el, "childNodes")).iter() {
            match get(&c, "nodeType").as_f64() {
                Some(3.0) => out.push(c),
                Some(1.0) if !call!(c, matches, SKIP_TEXT).is_truthy() => walk(&c, out),
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    walk(root, &mut out);
    out
}

/// Undo the wrapping (<https://developer.mozilla.org/en-US/docs/Web/API/Element/replaceWith>, <https://developer.mozilla.org/en-US/docs/Web/API/Node/normalize)>.
fn unwrap(root: &JsValue, base: &str) {
    let spans = Array::from(&call!(root, querySelectorAll, format!("span.{base}")));
    if spans.length() == 0 {
        return;
    }
    for s in spans.iter() {
        js::apply(&get(&s, "replaceWith"), &s, &Array::from(&get(&s, "childNodes")).iter().collect::<Vec<_>>());
    }
    call!(root, normalize);
}

/// Wrap the segments (start, end, mask), offsets into the nodes' joined text in UTF-16, in <span class="base class ...">,
/// splitting each text node where a segment starts or ends; a segment may cross nodes (part bold, part link).
/// (<https://developer.mozilla.org/en-US/docs/Web/API/DocumentFragment>, <https://developer.mozilla.org/en-US/docs/Web/API/CharacterData/data>)
fn apply_segments(nodes: &[JsValue], segs: &[Seg], m: &mut crate::Marks, hover: bool) {
    let doc = get(&js::global(), "document");
    let (mut offset, mut s) = (0u32, 0usize);
    for node in nodes {
        let data: JsString = get(node, "data").unchecked_into(); // a primitive string: JsString, not Reflect
        let (start, end) = (offset, offset + data.length());
        offset = end;
        while s < segs.len() && segs[s].to <= start {
            s += 1;
        }
        if s >= segs.len() || segs[s].from >= end {
            continue;
        }
        let frag = call!(doc, createDocumentFragment);
        let slice = |a: u32, e: u32| JsValue::from(data.slice(a, e));
        let mut at = 0;
        for (i, g) in segs.iter().enumerate().skip(s).take_while(|(_, g)| g.from < end) {
            let (a, e) = (g.from.max(start) - start, g.to.min(end) - start);
            if a > at {
                call!(frag, append, slice(at, a));
            }
            let span = call!(doc, createElement, "span");
            set(&span, "className", m.span_class(g.mask, g.strength));
            set(&span, "textContent", slice(a, e));
            if hover {
                set(&span, SEG_INDEX, i as u32);
            }
            call!(frag, append, span);
            at = e;
        }
        if at < end - start {
            call!(frag, append, slice(at, end - start));
        }
        call!(node, replaceWith, frag);
    }
}
