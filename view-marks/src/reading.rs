//! Reading view. Rendered blocks are registered as Obsidian renders them, but only those near the screen (an
//! IntersectionObserver, one screen either side) are sent to the worker, and the DOM is changed in animation frames
//! with a time budget, so wrapping never holds up scrolling. Elements are held weakly and forgotten when Obsidian
//! discards them (FinalizationRegistry). The worker maps each block to its source paragraph and returns segments with
//! the kinds over them; this file only splits text nodes.
//!
//! References: Obsidian's post-processors (<https://docs.obsidian.md/Plugins/Editor/Markdown+post+processing>),
//! <https://developer.mozilla.org/en-US/docs/Web/API/IntersectionObserver>, <https://developer.mozilla.org/en-US/docs/Web/API/Window/requestAnimationFrame>,
//! <https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/WeakRef>, <https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/FinalizationRegistry>.

use std::collections::{HashMap, HashSet, VecDeque};

use js_sys::{Array, JsString};
use wasm_bindgen::prelude::*;

use crate::app::{with, App};
use crate::js::{self, call, f1, get, new, obj, set, str};
use crate::msg::ToWorker;

const BLOCKS: &str = "p, li";
const SKIP_BLOCK: &str = "pre, table, .frontmatter-container, .internal-embed";
const SKIP_TEXT: &str = "code, .math, mjx-container, .internal-embed, sup.footnote-ref"; // text in a block that is not prose
const FRAME_BUDGET: f64 = 6.0; // ms of DOM work per animation frame

struct Rec {
    el: JsValue,      // WeakRef
    section: JsValue, // WeakRef
    ctx: JsValue,
    path: String,
    idx: u32,
    n: u32,
    text: Option<String>, // as sent; None until it first comes near the screen
    lines: Option<(u32, u32)>,
}

pub struct Reading {
    recs: HashMap<u32, Rec>,
    near: HashMap<String, HashSet<u32>>,
    queue: VecDeque<(u32, Vec<(u32, u32, u32)>)>,
    frame: bool,
    views_due: bool,
    next: u32,
    observer: JsValue,
    registry: JsValue,
}

fn weak(v: &JsValue) -> JsValue {
    new(&get(&js::global(), "WeakRef"), &[v.clone()])
}

impl Reading {
    pub fn new() -> Reading {
        let g = js::global();
        let observer = new(&get(&g, "IntersectionObserver"), &[
            f1(|entries| {
                with(|a| a.intersect(&entries));
                JsValue::UNDEFINED
            }),
            obj! { rootMargin: "100% 0px" },
        ]);
        let registry = new(&get(&g, "FinalizationRegistry"), &[f1(|id| {
            with(|a| a.forget(id.as_f64().unwrap_or(0.0) as u32));
            JsValue::UNDEFINED
        })]);
        Reading { recs: HashMap::new(), near: HashMap::new(), queue: VecDeque::new(), frame: false, views_due: false, next: 1, observer, registry }
    }

    pub fn queue(&mut self, block: u32, segs: Vec<(u32, u32, u32)>) {
        self.queue.retain(|(b, _)| *b != block);
        self.queue.push_back((block, segs));
        if !self.frame {
            self.frame = true;
            js::frame(|| _ = with(|a| a.drain()));
        }
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

    /// Every registered block's section takes its note's colour code, or none.
    pub fn tag(&self, code: impl Fn(&str) -> Option<String>) {
        for r in self.recs.values() {
            let section = call!(r.section, deref);
            if section.is_undefined() {
                continue;
            }
            match code(&r.path) {
                Some(c) => _ = call!(section, setAttribute, "data-av-code", c),
                None => _ = call!(section, removeAttribute, "data-av-code"),
            }
        }
    }

    pub fn clear(&mut self) {
        call!(self.observer, disconnect);
        for r in self.recs.values() {
            let el = call!(r.el, deref);
            if !el.is_undefined() {
                unwrap(&el);
            }
        }
        self.recs.clear();
        self.queue.clear();
    }
}

impl App {
    pub fn process(&mut self, section: JsValue, ctx: JsValue) {
        let found = if call!(section, matches, BLOCKS).is_truthy() { Array::of1(&section) } else { Array::from(&call!(section, querySelectorAll, BLOCKS)) };
        let blocks: Vec<JsValue> = found
            .iter()
            .filter(|b| call!(b, closest, SKIP_BLOCK).is_null() && !(str(&get(b, "tagName")) == "LI" && !call!(b, querySelector, "ul, ol, p").is_null()))
            .collect(); // a list item with nested blocks: its <p>s are taken instead
        if blocks.is_empty() {
            return;
        }
        let path = str(&get(&ctx, "sourcePath"));
        let Some(code) = self.code_id(&path) else { return }; // no colour code: nothing is sent
        call!(section, setAttribute, "data-av-code", code);
        self.ensure_doc(&path);
        let r = &mut self.reading;
        for (idx, b) in blocks.iter().enumerate() {
            let id = r.next;
            r.next += 1;
            set(b, "__av", id); // a property, not an attribute: no DOM change
            r.recs.insert(id, Rec { el: weak(b), section: weak(&section), ctx: ctx.clone(), path: path.clone(), idx: idx as u32, n: blocks.len() as u32, text: None, lines: None });
            call!(r.registry, register, b.clone(), id);
            call!(r.observer, observe, b.clone());
        }
    }

    fn intersect(&mut self, entries: &JsValue) {
        for e in Array::from(entries).iter() {
            let el = get(&e, "target");
            let id = get(&el, "__av").as_f64().unwrap_or(0.0) as u32;
            let Some(path) = self.reading.recs.get(&id).map(|r| r.path.clone()) else { continue };
            let near = self.reading.near.entry(path).or_default();
            if get(&e, "isIntersecting").is_truthy() {
                near.insert(id);
                if self.reading.recs[&id].text.is_none() {
                    self.register_block(id, &el);
                }
            } else {
                near.remove(&id);
            }
        }
        if !self.reading.views_due {
            self.reading.views_due = true;
            js::timeout(100.0, || _ = with(|a| a.send_views()));
        }
    }

    /// Tell the worker about a block once it comes near the screen (the DOM is settled by then). Its source lines:
    /// <https://docs.obsidian.md/Reference/TypeScript+API/MarkdownPostProcessorContext/getSectionInfo> (may be null).
    fn register_block(&mut self, id: u32, el: &JsValue) {
        let Some(r) = self.reading.recs.get_mut(&id) else { return };
        let section = call!(r.section, deref);
        let info = if section.is_undefined() { JsValue::NULL } else { call!(r.ctx, getSectionInfo, section) };
        let line = |k: &str| get(&info, k).as_f64().map(|v| v as u32);
        r.lines = if info.is_null() || info.is_undefined() { None } else { line("lineStart").zip(line("lineEnd")) };
        let text: String = text_nodes(el).iter().map(|n| str(&get(n, "data"))).collect();
        r.text = Some(text.clone());
        let m = ToWorker::Block { id, path: r.path.clone(), lines: r.lines, idx: r.idx, n: r.n, text };
        self.post(&m);
    }

    /// The lines near the screen in each note's Reading view, for the worker's plan.
    fn send_views(&mut self) {
        self.reading.views_due = false;
        let mut msgs = Vec::new();
        let coloured: HashSet<String> = self.reading.near.keys().filter(|p| self.code_id(p).is_some()).cloned().collect();
        self.reading.near.retain(|path, set| {
            let id = view_id(path);
            if !coloured.contains(path) {
                return true; // no colour code: not sent (retag dropped its view)
            }
            let lines = set.iter().filter_map(|b| self.reading.recs.get(b)?.lines);
            match lines.fold(None, |acc: Option<(u32, u32)>, (a, b)| Some(acc.map_or((a, b), |(x, y)| (x.min(a), y.max(b))))) {
                Some(vis) => msgs.push(ToWorker::View { id, path: path.clone(), editor: false, vis, render: (0, 0) }),
                None if set.is_empty() => {
                    msgs.push(ToWorker::DropView(id));
                    return false;
                }
                None => {}
            }
            true
        });
        msgs.iter().for_each(|m| self.post(m));
    }

    fn drain(&mut self) {
        self.reading.frame = false;
        let t0 = js::now();
        while let Some((id, segs)) = self.reading.queue.pop_front() {
            let Some(r) = self.reading.recs.get(&id) else { continue };
            let el = call!(r.el, deref);
            if !el.is_undefined() && get(&el, "isConnected").is_truthy() {
                unwrap(&el);
                let nodes = text_nodes(&el);
                if Some(nodes.iter().map(|n| str(&get(n, "data"))).collect::<String>()) != r.text {
                    self.register_block(id, &el); // re-rendered with other text: map again
                } else {
                    apply_segments(&nodes, &segs);
                }
            }
            if js::now() - t0 > FRAME_BUDGET {
                break; // the rest next frame
            }
        }
        if !self.reading.queue.is_empty() && !self.reading.frame {
            self.reading.frame = true;
            js::frame(|| _ = with(|a| a.drain()));
        }
    }

    fn forget(&mut self, id: u32) {
        if let Some(r) = self.reading.recs.remove(&id) {
            self.reading.near.get_mut(&r.path).map(|s| s.remove(&id));
        }
        self.post(&ToWorker::DropBlock(id));
    }
}

/// A Reading view's id for the worker, kept clear of the editors' small ids.
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
fn unwrap(root: &JsValue) {
    let spans = Array::from(&call!(root, querySelectorAll, "span.av"));
    if spans.length() == 0 {
        return;
    }
    for s in spans.iter() {
        js::apply(&get(&s, "replaceWith"), &s, &Array::from(&get(&s, "childNodes")).iter().collect::<Vec<_>>());
    }
    call!(root, normalize);
}

/// Wrap the segments (start, end, mask), offsets into the nodes' joined text in UTF-16, in <span class="av av-term ...">,
/// splitting each text node where a segment starts or ends; a segment may cross nodes (part bold, part link).
/// (<https://developer.mozilla.org/en-US/docs/Web/API/DocumentFragment>, <https://developer.mozilla.org/en-US/docs/Web/API/CharacterData/data>)
fn apply_segments(nodes: &[JsValue], segs: &[(u32, u32, u32)]) {
    let doc = get(&js::global(), "document");
    let (mut offset, mut s) = (0u32, 0usize);
    for node in nodes {
        let data: JsString = get(node, "data").unchecked_into(); // a primitive string: JsString, not Reflect
        let (start, end) = (offset, offset + data.length());
        offset = end;
        while s < segs.len() && segs[s].1 <= start {
            s += 1;
        }
        if s >= segs.len() || segs[s].0 >= end {
            continue;
        }
        let frag = call!(doc, createDocumentFragment);
        let slice = |a: u32, e: u32| JsValue::from(data.slice(a, e));
        let mut at = 0;
        for &(a, e, mask) in segs[s..].iter().take_while(|g| g.0 < end) {
            let (a, e) = (a.max(start) - start, e.min(end) - start);
            if a > at {
                call!(frag, append, slice(at, a));
            }
            let span = call!(doc, createElement, "span");
            let cls: Vec<String> = std::iter::once("av".to_string()).chain((0..crate::pick::KINDS as u8).filter(|k| mask & (1 << k) != 0).map(crate::pick::class)).collect();
            set(&span, "className", cls.join(" "));
            set(&span, "textContent", slice(a, e));
            call!(frag, append, span);
            at = e;
        }
        if at < end - start {
            call!(frag, append, slice(at, end - start));
        }
        call!(node, replaceWith, frag);
    }
}
