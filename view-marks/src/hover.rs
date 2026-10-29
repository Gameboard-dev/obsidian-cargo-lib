//! A pop-up over marks of chosen classes, in editors and Reading view: resting the pointer on a mark for
//! [`HoverSpec::delay_ms`] shows what the plugin says about it (a score's explanation, a definition). The content is
//! computed by the plugin's callback only on the first hover of a passage, then kept by a hash of its class and text
//! (the latest [`CACHE_MAX`]), so the same sentence in another note, or after an edit elsewhere, costs nothing. The
//! pop-up is one element, made on first use, filled as text (never as HTML), placed beside the mark inside the window,
//! and hidden when the pointer leaves, on scrolling and on a key press. Off unless [`install`] is called: nothing is
//! listened to otherwise.
//!
//! Which passage a mark is: in an editor, the mark's own range (the narrowest mark of a pop-up class at the pointer,
//! from the view plugin's decorations: <https://codemirror.net/docs/ref/#view.EditorView^findFromDOM>,
//! <https://codemirror.net/docs/ref/#view.EditorView.posAtDOM>, <https://codemirror.net/docs/ref/#state.RangeSet.between>);
//! in Reading view, the run of touching segments that carry the class ([`run_of`]).
//!
//! Its style is the plugin's: the element has the class `{base}-hover`, its parts `{base}-hover-title` and
//! `{base}-hover-body`; it is `position: fixed`, and set `left` and `top`.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;

use wasm_bindgen::prelude::*;
use xxhash_rust::xxh3::xxh3_64_with_seed;

use crate::js::{self, call, f1, f3, get, obj, set, str};
use crate::{Host, Seg};

/// Pop-up contents kept.
pub const CACHE_MAX: usize = 1000;
const GAP: f64 = 6.0; // px between the mark and the pop-up, and from the window's edge

/// What the pop-up shows: a title line and a body, as plain text.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Payload {
    pub title: String,
    pub body: String,
}

/// The plugin's callback: (class, the passage's text) to the pop-up's content, or None for no pop-up.
pub type PayloadFn = Rc<dyn Fn(u8, String) -> Pin<Box<dyn Future<Output = Option<Payload>>>>>;

/// Which marks have a pop-up, and what it shows.
pub struct HoverSpec {
    /// Classes (indices into [`crate::Config::classes`]) whose marks have a pop-up.
    pub classes: Vec<u8>,
    pub payload: PayloadFn,
    /// How long the pointer rests on a mark before the pop-up shows (ms).
    pub delay_ms: f64,
}

/// The pop-up contents computed, by key ([`key`]), the latest `max`.
pub struct Cache {
    map: HashMap<u64, Option<Rc<Payload>>>,
    order: VecDeque<u64>,
    max: usize,
}

impl Cache {
    pub fn new(max: usize) -> Cache {
        Cache { map: HashMap::new(), order: VecDeque::new(), max: max.max(1) }
    }

    /// Some(None): computed, and there is nothing to show.
    pub fn get(&self, key: u64) -> Option<Option<Rc<Payload>>> {
        self.map.get(&key).cloned()
    }

    pub fn put(&mut self, key: u64, p: Option<Payload>) {
        if self.map.insert(key, p.map(Rc::new)).is_none() {
            self.order.push_back(key);
        }
        while self.order.len() > self.max {
            if let Some(old) = self.order.pop_front() {
                self.map.remove(&old);
            }
        }
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

/// A passage's key: a hash of its text, seeded with its class (<https://docs.rs/xxhash-rust>).
pub fn key(class: u8, text: &str) -> u64 {
    xxh3_64_with_seed(text.as_bytes(), class as u64)
}

/// The narrowest of the marks at the pointer, (from, to, class), whose class has a pop-up.
pub fn narrowest(marks: &[(u32, u32, u8)], classes: &[u8]) -> Option<(u32, u32, u8)> {
    marks.iter().copied().filter(|m| classes.contains(&m.2) && m.0 < m.1).min_by_key(|m| (m.1 - m.0, m.0))
}

/// In Reading view, the passage of segment `i` for a pop-up: for each pop-up class over it, the run of touching
/// segments that carry that class; the narrowest run, as (from, to, class) in the block's UTF-16 offsets.
pub fn run_of(segs: &[Seg], i: usize, classes: &[u8]) -> Option<(u32, u32, u8)> {
    let g = segs.get(i)?;
    let runs = classes.iter().copied().filter(|&c| c < 32 && g.mask & (1 << c) != 0).map(|c| {
        let has = |j: usize| segs[j].mask & (1 << c) != 0;
        let (mut a, mut e) = (i, i);
        while a > 0 && segs[a - 1].to == segs[a].from && has(a - 1) {
            a -= 1;
        }
        while e + 1 < segs.len() && segs[e].to == segs[e + 1].from && has(e + 1) {
            e += 1;
        }
        (segs[a].from, segs[e].to, c)
    });
    runs.min_by_key(|r| (r.1 - r.0, r.0))
}

/// Where the pop-up goes, its top-left corner: below the mark (`rect`: left, top, right, bottom), or above it when
/// there is no room below, kept inside the window (`window`: width, height) with a small gap.
pub fn place(rect: (f64, f64, f64, f64), size: (f64, f64), window: (f64, f64)) -> (f64, f64) {
    let (left, top, _, bottom) = rect;
    let (w, h) = size;
    let x = left.min(window.0 - w - GAP).max(GAP);
    let below = bottom + GAP;
    let y = if below + h <= window.1 - GAP || top - GAP - h < GAP { below } else { top - GAP - h };
    (x, y.min(window.1 - h - GAP).max(GAP))
}

/// The pop-up's state, in [`crate::Marks`].
pub struct Hover {
    spec: HoverSpec,
    cache: Rc<RefCell<Cache>>,
    pending: Rc<RefCell<HashSet<u64>>>,
    el: JsValue,
    timer: Option<f64>,
    /// The passage under the pointer (its key), and the element it is drawn in.
    current: Option<u64>,
    target: JsValue,
}

impl Hover {
    pub(crate) fn remove(&self) {
        if let Some(t) = self.timer {
            js::clear_timeout(t);
        }
        if !self.el.is_undefined() {
            call!(self.el, remove);
        }
    }
}

/// Turn the pop-up on: `plugin` is the Obsidian plugin object, whose `registerDomEvent` removes the listeners when
/// it unloads (<https://docs.obsidian.md/Reference/TypeScript+API/Component/registerDomEvent>). Call it once, after
/// [`crate::Marks::new`].
pub fn install<H: Host>(h: &mut H, plugin: &JsValue, spec: HoverSpec) {
    h.marks().hover = Some(Hover {
        spec,
        cache: Rc::new(RefCell::new(Cache::new(CACHE_MAX))),
        pending: Rc::new(RefCell::new(HashSet::new())),
        el: JsValue::UNDEFINED,
        timer: None,
        current: None,
        target: JsValue::UNDEFINED,
    });
    let doc = get(&js::global(), "document");
    call!(plugin, registerDomEvent, doc.clone(), "mouseover", f1(|e| {
        H::with(|h| over(h, &get(&e, "target")));
        JsValue::UNDEFINED
    }));
    call!(plugin, registerDomEvent, doc.clone(), "mouseout", f1(|e| {
        if get(&e, "relatedTarget").is_null() {
            H::with(|h| hide(h)); // left the window
        }
        JsValue::UNDEFINED
    }));
    for event in ["keydown", "scroll"] {
        call!(plugin, registerDomEvent, doc.clone(), event, f1(|_| {
            H::with(|h| hide(h));
            JsValue::UNDEFINED
        }), obj! { capture: true, passive: true });
    }
}

/// The pointer is over `target`: a mark with a pop-up starts the delay, the pop-up itself keeps it, anything else hides it.
fn over<H: Host>(h: &mut H, target: &JsValue) {
    let m = h.marks();
    let Some(hv) = m.hover.as_ref() else { return };
    if !hv.el.is_undefined() && call!(hv.el, contains, target.clone()).is_truthy() {
        return;
    }
    let span = if call!(target, closest, "span").is_null() { JsValue::NULL } else { call!(target, closest, format!("span.{}", m.cfg.base)) };
    let found = if span.is_null() { None } else { passage(m, &span) };
    let Some((class, text)) = found else { return hide(h) };
    let k = key(class, &text);
    let hv = h.marks().hover.as_mut().unwrap_or_else(|| unreachable!());
    if hv.current == Some(k) {
        return;
    }
    hide(h);
    let hv = h.marks().hover.as_mut().unwrap_or_else(|| unreachable!());
    hv.current = Some(k);
    hv.target = span;
    hv.timer = Some(js::timeout(hv.spec.delay_ms, move || {
        H::with(|h| fetch::<H>(h, k, class, text));
    }));
}

/// The passage a mark's element shows, and its class: from the editor's decorations, or the block's segments.
fn passage(m: &crate::Marks, span: &JsValue) -> Option<(u8, String)> {
    let classes = &m.hover.as_ref()?.spec.classes;
    let cm = call!(span, closest, ".cm-editor");
    if !cm.is_null() {
        let view = call!(get(&m.cmv, "EditorView"), findFromDOM, cm);
        let pv = if view.is_null() || view.is_undefined() { JsValue::NULL } else { call!(view, plugin, m.plugin.clone()) };
        if pv.is_null() || pv.is_undefined() {
            return None;
        }
        let pos = call!(view, posAtDOM, span.clone(), 0).as_f64()? as u32;
        let found: Rc<RefCell<Vec<(u32, u32, u8)>>> = Rc::default();
        let (out, names, base) = (found.clone(), m.cfg.classes.clone(), m.cfg.base.clone());
        call!(get(&pv, "decorations"), between, pos, pos + 1, f3(move |from, to, value| {
            let spec = str(&get(&get(&value, "spec"), "class"));
            // "{base} {class}[ {base}-s{n}]": the class is the second word
            let name = spec.split(' ').nth(1).unwrap_or("");
            if let Some(c) = names.iter().position(|n| n == name).filter(|_| spec.starts_with(&base)) {
                let n = |v: &JsValue| v.as_f64().unwrap_or(0.0) as u32;
                out.borrow_mut().push((n(&from), n(&to), c as u8));
            }
            JsValue::UNDEFINED
        }));
        let (from, to, class) = narrowest(&found.borrow(), classes)?;
        let text = str(&call!(get(&view, "state"), sliceDoc, from, to));
        return Some((class, text));
    }
    let i = get(span, crate::reading::SEG_INDEX).as_f64()? as usize;
    let mut el = get(span, "parentElement");
    let block = loop {
        if el.is_null() || el.is_undefined() {
            return None;
        }
        if let Some(id) = get(&el, crate::reading::BLOCK_ID).as_f64() {
            break id as u32;
        }
        el = get(&el, "parentElement");
    };
    let rec = m.reading.recs.get(&block)?;
    let (from, to, class) = run_of(&rec.segs, i, classes)?;
    let text: Vec<u16> = rec.text.as_deref()?.encode_utf16().skip(from as usize).take((to - from) as usize).collect();
    Some((class, String::from_utf16_lossy(&text)))
}

/// The delay passed: show the cached content, or compute it (once, however often it is hovered meanwhile).
fn fetch<H: Host>(h: &mut H, k: u64, class: u8, text: String) {
    let Some(hv) = h.marks().hover.as_mut() else { return };
    hv.timer = None;
    if hv.current != Some(k) {
        return;
    }
    let cached = hv.cache.borrow().get(k);
    if let Some(p) = cached {
        if let Some(p) = p {
            show(h, &p);
        }
        return;
    }
    if !hv.pending.borrow_mut().insert(k) {
        return;
    }
    let (cache, pending, fut) = (hv.cache.clone(), hv.pending.clone(), (hv.spec.payload)(class, text));
    js::spawn(async move {
        let p = fut.await;
        cache.borrow_mut().put(k, p.clone());
        pending.borrow_mut().remove(&k);
        if let Some(p) = p {
            H::with(|h| {
                if h.marks().hover.as_ref().is_some_and(|hv| hv.current == Some(k)) {
                    show(h, &p);
                }
            });
        }
    });
}

fn show<H: Host>(h: &mut H, p: &Payload) {
    let base = h.marks().cfg.base.clone();
    let Some(hv) = h.marks().hover.as_mut() else { return };
    let doc = get(&js::global(), "document");
    if hv.el.is_undefined() {
        let el = call!(doc, createElement, "div");
        set(&el, "className", format!("{base}-hover"));
        call!(get(&doc, "body"), appendChild, el.clone());
        hv.el = el;
    }
    let el = &hv.el;
    call!(el, replaceChildren);
    for (part, text) in [("title", &p.title), ("body", &p.body)] {
        if !text.is_empty() {
            let d = call!(doc, createElement, "div");
            set(&d, "className", format!("{base}-hover-{part}"));
            set(&d, "textContent", text.as_str());
            call!(el, appendChild, d);
        }
    }
    let style = get(el, "style");
    set(&style, "display", "block");
    let r = call!(hv.target, getBoundingClientRect);
    let n = |v: &JsValue, k: &str| get(v, k).as_f64().unwrap_or(0.0);
    let g = js::global();
    let (x, y) = place((n(&r, "left"), n(&r, "top"), n(&r, "right"), n(&r, "bottom")), (n(el, "offsetWidth"), n(el, "offsetHeight")), (n(&g, "innerWidth"), n(&g, "innerHeight")));
    set(&style, "left", format!("{x}px"));
    set(&style, "top", format!("{y}px"));
}

fn hide<H: Host>(h: &mut H) {
    let Some(hv) = h.marks().hover.as_mut() else { return };
    if let Some(t) = hv.timer.take() {
        js::clear_timeout(t);
    }
    hv.current = None;
    hv.target = JsValue::UNDEFINED;
    if !hv.el.is_undefined() {
        set(&get(&hv.el, "style"), "display", "none");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(from: u32, to: u32, classes: &[u8]) -> Seg {
        Seg { from, to, mask: classes.iter().fold(0, |m, c| m | (1 << c)), strength: 0 }
    }

    #[test]
    fn a_sentence_split_by_a_phrase_is_one_passage_and_the_phrase_another() {
        // a sentence (class 3) over [0, 30), a phrase (class 1) over [10, 15), then a gap and another sentence
        let segs = [seg(0, 10, &[3]), seg(10, 15, &[3, 1]), seg(15, 30, &[3]), seg(31, 40, &[3])];
        assert_eq!(run_of(&segs, 0, &[3]), Some((0, 30, 3)));
        assert_eq!(run_of(&segs, 2, &[3]), Some((0, 30, 3)));
        assert_eq!(run_of(&segs, 1, &[3, 1]), Some((10, 15, 1)), "the narrowest: the phrase");
        assert_eq!(run_of(&segs, 1, &[3]), Some((0, 30, 3)));
        assert_eq!(run_of(&segs, 3, &[3]), Some((31, 40, 3)), "not touching: another passage");
        assert_eq!(run_of(&segs, 0, &[1]), None, "no pop-up class there");
        assert_eq!(run_of(&segs, 9, &[3]), None);
    }

    #[test]
    fn the_narrowest_mark_with_a_pop_up() {
        let marks = [(0, 30, 3), (10, 15, 1), (12, 14, 0)];
        assert_eq!(narrowest(&marks, &[3, 1]), Some((10, 15, 1)));
        assert_eq!(narrowest(&marks, &[3]), Some((0, 30, 3)));
        assert_eq!(narrowest(&marks, &[5]), None);
    }

    #[test]
    fn the_cache_keeps_the_latest_and_knows_nothing_to_show() {
        let mut c = Cache::new(2);
        let (a, b, d) = (key(1, "a"), key(1, "b"), key(2, "a"));
        assert_ne!(a, d, "the class is part of the key");
        assert_eq!(c.get(a), None);
        c.put(a, Some(Payload { title: "t".into(), body: "x".into() }));
        c.put(b, None);
        assert_eq!(c.get(b), Some(None), "computed: nothing to show");
        assert_eq!(c.get(a).flatten().map(|p| p.body.clone()), Some("x".into()));
        c.put(d, None);
        assert_eq!((c.get(a), c.len()), (None, 2), "the oldest dropped");
    }

    #[test]
    fn the_pop_up_stays_in_the_window() {
        let win = (800.0, 600.0);
        assert_eq!(place((100.0, 100.0, 200.0, 120.0), (300.0, 100.0), win), (100.0, 126.0), "below");
        assert_eq!(place((100.0, 500.0, 200.0, 520.0), (300.0, 100.0), win), (100.0, 394.0), "no room below: above");
        assert_eq!(place((700.0, 100.0, 760.0, 120.0), (300.0, 100.0), win), (494.0, 126.0), "kept off the right edge");
        assert_eq!(place((10.0, 10.0, 50.0, 30.0), (300.0, 700.0), win), (10.0, 6.0), "taller than the window: from its top");
    }
}
