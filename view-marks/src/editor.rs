//! Live Preview and Source mode: a CodeMirror view plugin that tells the host what the editor shows and draws the
//! marks it returns. It never scans the note: an edit maps the existing marks through the change (CodeMirror's own
//! mapping), the text goes to the host only after typing pauses, and the visible range at most once a frame.
//!
//! References: Obsidian's editor extensions (<https://docs.obsidian.md/Plugins/Editor/Editor+extensions>), CodeMirror's
//! decorations (<https://codemirror.net/examples/decoration/>) and reference manual (<https://codemirror.net/docs/ref/>).

use wasm_bindgen::prelude::*;

use crate::js::{self, call, f0, f1, get, new, obj, set, str};
use crate::{Event, Host, Mark};

const DOC_DELAY: f64 = 400.0; // ms of quiet after typing before the host gets the new text

pub struct Ed {
    pub(crate) view: JsValue,
    value: JsValue, // the plugin value CodeMirror holds: { decorations, update, destroy }
    pub(crate) path: String,
    sent: u32,     // the version of the text last sent
    edited: bool,  // changed since: marks computed on the old text are not applied
    timer: Option<f64>,
    frame: bool,
}

/// `ViewPlugin.define(view => value, { decorations: v => v.decorations })` (<https://codemirror.net/docs/ref/#view.ViewPlugin^define>,
/// <https://codemirror.net/docs/ref/#view.PluginSpec.decorations)>. Register what it returns with Obsidian's
/// `registerEditorExtension` (<https://docs.obsidian.md/Reference/TypeScript+API/Plugin/registerEditorExtension>).
pub fn extension<H: Host>(h: &mut H) -> JsValue {
    let cmv = h.marks().cmv.clone();
    let none = get(&get(&cmv, "Decoration"), "none");
    let create = f1(move |view| {
        let value = obj! { decorations: none.clone() };
        let id = H::with(|h| add_editor(h, view, value.clone())).unwrap_or(0);
        set(&value, "update", f1(move |u| {
            H::with(|h| update_editor(h, id, &u));
            JsValue::UNDEFINED
        }));
        set(&value, "destroy", f0(move || _ = H::with(|h| drop_editor(h, id))));
        value
    });
    let plugin = call!(get(&cmv, "ViewPlugin"), define, create, obj! { decorations: f1(|v| get(&v, "decorations")) });
    h.marks().plugin = plugin.clone();
    plugin
}

/// The host's marks for an editor (`id`, as its [`Event::View`] said), made for the text of `version`: drawn, unless
/// the text was edited since. Call it outside the host's state (not inside [`Host::with`]): the dispatch runs the
/// editor's update at once. (<https://codemirror.net/docs/ref/#state.RangeSetBuilder>, <https://codemirror.net/docs/ref/#view.Decoration^mark>, <https://codemirror.net/docs/ref/#state.StateEffect^define>,
/// <https://codemirror.net/docs/ref/#view.EditorView.dispatch>)
///
/// The marks must be sorted by `from` (CodeMirror's builder asks for that), and, at one `from`, by the order they are
/// to nest in.
pub fn apply<H: Host>(id: u32, version: u32, marks: &[Mark]) {
    let Some(Some((view, value, cms, decos, refresh))) = H::with(|h| {
        let m = h.marks();
        let ed = m.editors.get(&id)?;
        if ed.edited || version < ed.sent {
            return None;
        }
        let (view, value) = (ed.view.clone(), ed.value.clone());
        let decos: Vec<JsValue> = marks.iter().map(|k| m.deco(k.class, k.strength)).collect(); // made once per class and step
        Some((view, value, m.cms.clone(), decos, m.refresh.clone()))
    }) else { return };
    let len = get(&get(&get(&view, "state"), "doc"), "length").as_f64().unwrap_or(0.0) as u32;
    let b = new(&get(&cms, "RangeSetBuilder"), &[]);
    for (m, deco) in marks.iter().zip(decos).filter(|(m, d)| m.to <= len && !d.is_undefined()) {
        call!(b, add, m.from, m.to, deco);
    }
    set(&value, "decorations", call!(b, finish));
    call!(view, dispatch, obj! { effects: call!(refresh, of, JsValue::NULL) });
}

/// The note an editor shows: Obsidian's <https://docs.obsidian.md/Reference/TypeScript+API/editorInfoField>.
fn editor_path<H: Host>(h: &mut H, view: &JsValue) -> String {
    let info = call!(get(view, "state"), field, get(&h.marks().obs, "editorInfoField"), false);
    str(&get(&get(&info, "file"), "path"))
}

fn add_editor<H: Host>(h: &mut H, view: JsValue, value: JsValue) -> u32 {
    let path = editor_path(h, &view);
    let m = h.marks();
    let id = m.next_id;
    m.next_id += 1;
    m.editors.insert(id, Ed { view, value, path, sent: 0, edited: false, timer: None, frame: false });
    tag_editor(h, id);
    editor_view(h, id);
    id
}

/// CodeMirror's update (<https://codemirror.net/docs/ref/#view.ViewUpdate>): marks follow an edit through `RangeSet.map`
/// (<https://codemirror.net/docs/ref/#state.RangeSet.map>).
fn update_editor<H: Host>(h: &mut H, id: u32, u: &JsValue) {
    let path = editor_path(h, &get(u, "view"));
    let m = h.marks();
    let none = get(&get(&m.cmv, "Decoration"), "none");
    let Some(ed) = m.editors.get_mut(&id) else { return };
    let changed = get(u, "docChanged").is_truthy();
    if path != ed.path { // another note in this editor
        ed.path = path;
        set(&ed.value, "decorations", none);
        tag_editor(h, id);
        return;
    } else if changed {
        set(&ed.value, "decorations", call!(get(&ed.value, "decorations"), map, get(u, "changes")));
        ed.edited = true;
        if let Some(t) = ed.timer.take() {
            js::clear_timeout(t);
        }
        ed.timer = Some(js::timeout(DOC_DELAY, move || _ = H::with(|h| editor_doc(h, id))));
    }
    let Some(ed) = h.marks().editors.get_mut(&id) else { return };
    if !ed.frame && (changed || get(u, "viewportChanged").is_truthy() || get(u, "geometryChanged").is_truthy()) {
        ed.frame = true;
        js::frame(move || _ = H::with(|h| editor_view(h, id)));
    }
}

/// The editor takes its note's scope (the [`crate::Config::attr`] attribute on its DOM, which the host's stylesheet
/// scopes to), and is sent to the host; with no scope it is not, and shows nothing
/// (<https://codemirror.net/docs/ref/#view.EditorView.dom>). Call it for every editor ([`editor_ids`]) when what
/// [`Host::scope`] answers may have changed.
pub fn tag_editor<H: Host>(h: &mut H, id: u32) {
    let Some(ed) = h.marks().editors.get(&id) else { return };
    let (dom, path) = (get(&ed.view, "dom"), ed.path.clone());
    let code = h.scope(&path);
    let attr = h.marks().cfg.attr.clone();
    match &code {
        Some(c) => _ = call!(dom, setAttribute, attr, c.clone()),
        None => _ = call!(dom, removeAttribute, attr),
    }
    if code.is_some() {
        editor_doc(h, id);
        editor_view(h, id);
    } else {
        h.send(Event::DropView(id));
    }
}

/// The editors open now.
pub fn editor_ids<H: Host>(h: &mut H) -> Vec<u32> {
    h.marks().editors.keys().copied().collect()
}

fn editor_doc<H: Host>(h: &mut H, id: u32) {
    let Some(ed) = h.marks().editors.get_mut(&id) else { return };
    ed.timer = None;
    if ed.path.is_empty() {
        return;
    }
    let path = ed.path.clone();
    if h.scope(&path).is_none() {
        return; // not marked: nothing is sent
    }
    let Some(ed) = h.marks().editors.get_mut(&id) else { return };
    let (path, text) = (ed.path.clone(), str(&call!(get(&get(&ed.view, "state"), "doc"), toString)));
    let v = h.send_doc(&path, text);
    if let Some(ed) = h.marks().editors.get_mut(&id) {
        (ed.sent, ed.edited) = (v, false);
    }
}

/// What the editor shows (<https://codemirror.net/docs/ref/#view.EditorView.visibleRanges>) and renders (<https://codemirror.net/docs/ref/#view.EditorView.viewport>).
fn editor_view<H: Host>(h: &mut H, id: u32) {
    let Some(ed) = h.marks().editors.get_mut(&id) else { return };
    ed.frame = false;
    let n = |v: &JsValue, k: &str| get(v, k).as_f64().unwrap_or(0.0) as u32;
    let (vis, port) = (js_sys::Array::from(&get(&ed.view, "visibleRanges")), get(&ed.view, "viewport"));
    let render = (n(&port, "from"), n(&port, "to"));
    let visible = if vis.length() == 0 { render } else { (n(&vis.get(0), "from"), n(&vis.get(vis.length() - 1), "to")) };
    let m = Event::View { id, path: ed.path.clone(), editor: true, vis: visible, render };
    let path = ed.path.clone();
    if !path.is_empty() && h.scope(&path).is_some() {
        h.send(m);
    }
}

fn drop_editor<H: Host>(h: &mut H, id: u32) {
    if let Some(t) = h.marks().editors.remove(&id).and_then(|e| e.timer) {
        js::clear_timeout(t);
    }
    h.send(Event::DropView(id));
}
