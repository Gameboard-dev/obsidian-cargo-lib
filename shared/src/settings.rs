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
    /// A link (the plugin's README, say), shown after the description and opened in the browser.
    Link(&'static str),
    /// Anything else: the plugin fills the Setting it is given.
    Custom(fn(&JsValue)),
}

pub type Row = (&'static str, &'static str, &'static str, Ctl);

/// Fill `tab` (a PluginSettingTab) with `rows`: each control shows `get(key)` and calls `set(key, value)` on a change
/// (numbers as numbers, text trimmed).
pub fn render(tab: &JsValue, obsidian: &JsValue, rows: &'static [Row], get_value: impl Fn(&str) -> JsValue, set_value: fn(&'static str, JsValue)) {
    let el = get(tab, "containerEl");
    call!(el, empty);
    let setting = get(obsidian, "Setting");
    for (key, name, desc, ctl) in rows {
        let (key, s) = (*key, new(&setting, &[el.clone()]));
        call!(s, setName, *name);
        call!(s, setDesc, *desc);
        let value = get_value(key);
        let changed = move |v: JsValue| {
            set_value(key, v);
            JsValue::UNDEFINED
        };
        match ctl {
            Ctl::Head => _ = call!(s, setHeading),
            Ctl::Note => {}
            Ctl::Link(url) => {
                let a = call!(get(&crate::js::global(), "document"), createElement, "a");
                set(&a, "href", *url);
                set(&a, "textContent", *url);
                call!(get(&s, "descEl"), appendChild, a);
            }
            Ctl::Custom(fill) => fill(&s),
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
    }
}
