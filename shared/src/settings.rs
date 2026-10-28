//! A plugin's settings tab from a table of rows: (key, name, description, control). Values are read and written by
//! key through the plugin's own functions, so the table is all a plugin writes.
//!
//! Obsidian's API: <https://docs.obsidian.md/Reference/TypeScript+API/PluginSettingTab> and
//! <https://docs.obsidian.md/Reference/TypeScript+API/Setting> (setHeading, addToggle, addSlider, addText, addDropdown).

use wasm_bindgen::prelude::*;

use crate::js::{call, f1, get, new, set, str};

pub enum Ctl {
    Head,
    Note,
    Toggle,
    Slider(f64, f64, f64),
    Text,
    Number,
    /// A dropdown of (value, label) pairs.
    Choice(fn() -> Vec<(&'static str, &'static str)>),
    /// A dropdown of (value, label) pairs given as a table (a settings schema's).
    Options(&'static [(&'static str, &'static str)]),
    /// A link (the plugin's README, say), shown after the description and opened in the browser.
    Link(&'static str),
    /// Anything else: the plugin fills the Setting it is given.
    Custom(fn(&JsValue)),
}

pub type Row = (&'static str, &'static str, &'static str, Ctl);

/// Fill `tab` (a PluginSettingTab) with `rows`: each control shows `get(key)` and calls `set(key, value)` on a change
/// (numbers as numbers, text trimmed).
pub fn render(tab: &JsValue, obsidian: &JsValue, rows: &'static [Row], get_value: impl Fn(&str) -> JsValue, set_value: fn(&'static str, JsValue)) {
    render_with(tab, obsidian, rows, get_value, set_value, |_| None);
}

/// `render`, with what the plugin knows only while it runs: for a key, `state` may return whether its control is
/// disabled (Setting.setDisabled) and a sentence added to its description (why, or what was found).
pub fn render_with(
    tab: &JsValue,
    obsidian: &JsValue,
    rows: &'static [Row],
    get_value: impl Fn(&str) -> JsValue,
    set_value: fn(&'static str, JsValue),
    state: impl Fn(&str) -> Option<(bool, String)>,
) {
    let el = get(tab, "containerEl");
    call!(el, empty);
    let setting = get(obsidian, "Setting");
    // a tab with several sections opens with their contents: a link to each, which scrolls to its heading
    let heads: Vec<&str> = rows.iter().filter(|r| matches!(r.3, Ctl::Head)).map(|r| r.1).collect();
    let targets: std::rc::Rc<std::cell::RefCell<Vec<JsValue>>> = Default::default();
    if heads.len() >= 3 {
        let doc = get(&crate::js::global(), "document");
        let c = new(&setting, &[el.clone()]);
        // not "Settings contents": Obsidian's plugin guidelines ask for no "settings" in a settings tab's headings
        call!(call!(c, setName, "Contents"), setHeading);
        let list = call!(doc, createElement, "div");
        set(&list, "className", "setting-contents");
        for (i, name) in heads.iter().enumerate() {
            let a = call!(doc, createElement, "a");
            set(&a, "textContent", *name);
            set(&a, "href", "#");
            let t = targets.clone();
            call!(a, addEventListener, "click", f1(move |e| {
                call!(e, preventDefault);
                if let Some(h) = t.borrow().get(i) {
                    call!(h, scrollIntoView, crate::js::obj! { behavior: "smooth", block: "start" });
                }
                JsValue::UNDEFINED
            }));
            call!(list, appendChild, a);
            if i + 1 < heads.len() {
                call!(list, appendChild, call!(doc, createTextNode, " \u{b7} "));
            }
        }
        call!(get(&c, "descEl"), appendChild, list);
    }
    for (key, name, desc, ctl) in rows {
        let (key, s) = (*key, new(&setting, &[el.clone()]));
        call!(s, setName, *name);
        let (disabled, extra) = state(key).unwrap_or_default();
        call!(s, setDesc, if extra.is_empty() { desc.to_string() } else { format!("{desc} {extra}").trim().to_string() });
        let value = get_value(key);
        let changed = move |v: JsValue| {
            set_value(key, v);
            JsValue::UNDEFINED
        };
        match ctl {
            Ctl::Head => {
                call!(s, setHeading);
                targets.borrow_mut().push(get(&s, "settingEl"));
            }
            Ctl::Note => {}
            Ctl::Link(url) => {
                let a = call!(get(&crate::js::global(), "document"), createElement, "a");
                set(&a, "href", *url);
                set(&a, "textContent", *url);
                call!(get(&s, "descEl"), appendChild, a);
            }
            Ctl::Custom(fill) => fill(&s),
            Ctl::Options(options) => {
                let options = *options;
                call!(s, addDropdown, f1(move |d| {
                    for (v, label) in options {
                        call!(d, addOption, *v, *label);
                    }
                    call!(call!(d, setValue, value.clone()), onChange, f1(changed))
                }));
            }
            Ctl::Choice(options) => {
                let options = *options;
                call!(s, addDropdown, f1(move |d| {
                    for (v, label) in options() {
                        call!(d, addOption, v, label);
                    }
                    call!(call!(d, setValue, value.clone()), onChange, f1(changed))
                }));
            }
            Ctl::Toggle => _ = call!(s, addToggle, f1(move |t| call!(call!(t, setValue, value.clone()), onChange, f1(changed)))),
            &Ctl::Slider(lo, hi, step) => _ = call!(s, addSlider, f1(move |sl| {
                call!(sl, setLimits, lo, hi, step);
                call!(sl, setDynamicTooltip);
                call!(call!(sl, setValue, value.clone()), onChange, f1(changed))
            })),
            Ctl::Text | Ctl::Number => {
                let number = matches!(ctl, Ctl::Number);
                let text = value.as_string().unwrap_or_else(|| value.as_f64().map(|n| n.to_string()).unwrap_or_default());
                call!(s, addText, f1(move |t| {
                    call!(t, setValue, text.clone());
                    call!(t, onChange, f1(move |v| changed(if number { str(&v).trim().parse::<f64>().unwrap_or(0.0).into() } else { str(&v).trim().into() })))
                }));
            }
        }
        if disabled {
            call!(s, setDisabled, true);
        }
    }
}
