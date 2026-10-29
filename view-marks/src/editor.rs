//! Live Preview and Source mode: a CodeMirror view plugin that tells the worker what the editor shows and draws the
//! marks it returns. It never scans the note: an edit maps the existing marks through the change (CodeMirror's own
//! mapping), the text goes to the worker only after typing pauses, and the visible range at most once a frame.
//!
//! References: Obsidian's editor extensions (<https://docs.obsidian.md/Plugins/Editor/Editor+extensions>), CodeMirror's
//! decorations (<https://codemirror.net/examples/decoration/>) and reference manual (<https://codemirror.net/docs/ref/>).

use wasm_bindgen::prelude::*;

use crate::app::{with, App};
use crate::js::{self, call, f0, f1, get, new, obj, set, str};
use crate::msg::ToWorker;

const DOC_DELAY: f64 = 400.0; // ms of quiet after typing before the worker gets the new text

pub struct Ed {
    view: JsValue,
    value: JsValue, // the plugin value CodeMirror holds: { decorations, update, destroy }
    path: String,
    sent: u32,     // the version of the text last sent
    edited: bool,  // changed since: marks computed on the old text are not applied
    timer: Option<f64>,
    frame: bool,
}

/// `ViewPlugin.define(view => value, { decorations: v => v.decorations })` (<https://codemirror.net/docs/ref/#view.ViewPlugin^define>,
/// <https://codemirror.net/docs/ref/#view.PluginSpec.decorations)>.
pub fn extension(cmv: &JsValue) -> JsValue {
    let none = get(&get(cmv, "Decoration"), "none");
    let create = f1(move |view| {
        let value = obj! { decorations: none.clone() };
        let id = with(|a| a.add_editor(view, value.clone())).unwrap_or(0);
        set(&value, "update", f1(move |u| {
            with(|a| a.update_editor(id, &u));
            JsValue::UNDEFINED
        }));
        set(&value, "destroy", f0(move || _ = with(|a| a.drop_editor(id))));
        value
    });
    call!(get(cmv, "ViewPlugin"), define, create, obj! { decorations: f1(|v| get(&v, "decorations")) })
}

/// Marks from the worker. Built and dispatched outside `with`: the dispatch runs the editor's update at once.
/// (<https://codemirror.net/docs/ref/#state.RangeSetBuilder>, <https://codemirror.net/docs/ref/#view.Decoration^mark>, <https://codemirror.net/docs/ref/#state.StateEffect^define>,
/// <https://codemirror.net/docs/ref/#view.EditorView.dispatch>)
pub fn apply(id: u32, version: u32, marks: &[(u32, u32, u8)]) {
    let Some(Some((view, value, cms, decos, refresh))) = with(|a| {
        let ed = a.editors.get(&id)?;
        (!ed.edited && version >= ed.sent).then(|| (ed.view.clone(), ed.value.clone(), a.cms.clone(), a.marks.clone(), a.refresh.clone()))
    }) else { return };
    let len = get(&get(&get(&view, "state"), "doc"), "length").as_f64().unwrap_or(0.0) as u32;
    let b = new(&get(&cms, "RangeSetBuilder"), &[]);
    for &(from, to, k) in marks.iter().filter(|m| m.1 <= len) {
        call!(b, add, from, to, decos[k as usize].clone());
    }
    set(&value, "decorations", call!(b, finish));
    call!(view, dispatch, obj! { effects: call!(refresh, of, JsValue::NULL) });
}

impl App {
    /// The note an editor shows: Obsidian's <https://docs.obsidian.md/Reference/TypeScript+API/editorInfoField>.
    fn editor_path(&self, view: &JsValue) -> String {
        let info = call!(get(view, "state"), field, get(&self.obs, "editorInfoField"), false);
        str(&get(&get(&info, "file"), "path"))
    }

    fn add_editor(&mut self, view: JsValue, value: JsValue) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        let path = self.editor_path(&view);
        self.editors.insert(id, Ed { view, value, path, sent: 0, edited: false, timer: None, frame: false });
        self.tag_editor(id);
        self.editor_view(id);
        id
    }

    /// CodeMirror's update (<https://codemirror.net/docs/ref/#view.ViewUpdate>): marks follow an edit through `RangeSet.map`
    /// (<https://codemirror.net/docs/ref/#state.RangeSet.map>).
    fn update_editor(&mut self, id: u32, u: &JsValue) {
        let path = self.editor_path(&get(u, "view"));
        let none = get(&get(&self.cmv, "Decoration"), "none");
        let Some(ed) = self.editors.get_mut(&id) else { return };
        let changed = get(u, "docChanged").is_truthy();
        if path != ed.path { // another note in this editor
            ed.path = path;
            set(&ed.value, "decorations", none);
            self.tag_editor(id);
            return;
        } else if changed {
            set(&ed.value, "decorations", call!(get(&ed.value, "decorations"), map, get(u, "changes")));
            ed.edited = true;
            if let Some(t) = ed.timer.take() {
                js::clear_timeout(t);
            }
            ed.timer = Some(js::timeout(DOC_DELAY, move || _ = with(|a| a.editor_doc(id))));
        }
        let Some(ed) = self.editors.get_mut(&id) else { return };
        if !ed.frame && (changed || get(u, "viewportChanged").is_truthy() || get(u, "geometryChanged").is_truthy()) {
            ed.frame = true;
            js::frame(move || _ = with(|a| a.editor_view(id)));
        }
    }

    /// The editor takes its note's colour code (`data-av-code` on its DOM, which the stylesheet scopes to), and is
    /// sent to the worker; with no code it is not, and shows nothing (<https://codemirror.net/docs/ref/#view.EditorView.dom>).
    pub fn tag_editor(&mut self, id: u32) {
        let Some(ed) = self.editors.get(&id) else { return };
        let (dom, code) = (get(&ed.view, "dom"), self.code_id(&ed.path));
        match &code {
            Some(c) => _ = call!(dom, setAttribute, "data-av-code", c.clone()),
            None => _ = call!(dom, removeAttribute, "data-av-code"),
        }
        if code.is_some() {
            self.editor_doc(id);
            self.editor_view(id);
        } else {
            self.post(&ToWorker::DropView(id));
        }
    }

    fn editor_doc(&mut self, id: u32) {
        let Some(ed) = self.editors.get_mut(&id) else { return };
        ed.timer = None;
        if ed.path.is_empty() {
            return;
        }
        let path = ed.path.clone();
        if self.code_id(&path).is_none() {
            return; // no colour code: nothing is sent
        }
        let Some(ed) = self.editors.get_mut(&id) else { return };
        let (path, text) = (ed.path.clone(), str(&call!(get(&get(&ed.view, "state"), "doc"), toString)));
        let v = self.send_doc(&path, text);
        if let Some(ed) = self.editors.get_mut(&id) {
            (ed.sent, ed.edited) = (v, false);
        }
    }

    /// What the editor shows (<https://codemirror.net/docs/ref/#view.EditorView.visibleRanges>) and renders (<https://codemirror.net/docs/ref/#view.EditorView.viewport>).
    fn editor_view(&mut self, id: u32) {
        let Some(ed) = self.editors.get_mut(&id) else { return };
        ed.frame = false;
        let n = |v: &JsValue, k: &str| get(v, k).as_f64().unwrap_or(0.0) as u32;
        let (vis, port) = (js_sys::Array::from(&get(&ed.view, "visibleRanges")), get(&ed.view, "viewport"));
        let render = (n(&port, "from"), n(&port, "to"));
        let visible = if vis.length() == 0 { render } else { (n(&vis.get(0), "from"), n(&vis.get(vis.length() - 1), "to")) };
        let m = ToWorker::View { id, path: ed.path.clone(), editor: true, vis: visible, render };
        let path = ed.path.clone();
        if !path.is_empty() && self.code_id(&path).is_some() {
            self.post(&m);
        }
    }

    fn drop_editor(&mut self, id: u32) {
        if let Some(t) = self.editors.remove(&id).and_then(|e| e.timer) {
            js::clear_timeout(t);
        }
        self.post(&ToWorker::DropView(id));
    }
}
