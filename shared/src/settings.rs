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
    call!(get(&el, "classList"), remove, "setting-contents-layout");
    let setting = get(obsidian, "Setting");
    // a tab with several sections opens with their contents: a link to each, which scrolls to its heading. Filled
    // once the tab is drawn, so that headings a plugin adds after these rows (Quarry's keys, say) are listed too.
    let nav = call!(get(&crate::js::global(), "document"), createElement, "nav");
    set(&nav, "className", "setting-contents");
    call!(el, appendChild, nav.clone());
    let tab_el = el.clone();
    crate::js::timeout(0.0, move || contents(&tab_el, &nav));
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
            Ctl::Head => _ = call!(s, setHeading),
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

/// Fill `nav` with a link to every heading in the settings tab `el`, when it has three or more. The tab gets the
/// class `setting-contents-layout`; a plugin's styles.css may then set the contents beside the settings, in view as
/// they scroll (see quarry/styles.css). Without those rules the links show in a line above the settings.
fn contents(el: &JsValue, nav: &JsValue) {
    // the tab was drawn again, or closed, before this ran
    if get(nav, "isConnected") != JsValue::TRUE {
        return;
    }
    let heads = call!(el, querySelectorAll, ".setting-item-heading");
    let n = get(&heads, "length").as_f64().unwrap_or(0.0) as u32;
    if n < 3 {
        call!(nav, remove);
        return;
    }
    let doc = get(&crate::js::global(), "document");
    let title = call!(doc, createElement, "div");
    set(&title, "className", "setting-contents-title");
    // not "Settings contents": Obsidian's plugin guidelines ask for no "settings" in a settings tab's headings
    set(&title, "textContent", "Contents");
    call!(nav, appendChild, title);
    let links = call!(doc, createElement, "div");
    set(&links, "className", "setting-contents-links");
    for i in 0..n {
        let head = call!(heads, item, i);
        let name = call!(head, querySelector, ".setting-item-name");
        let a = call!(doc, createElement, "a");
        set(&a, "textContent", if name.is_null() { String::new() } else { str(&get(&name, "textContent")) });
        set(&a, "href", "#");
        call!(a, addEventListener, "click", f1(move |e| {
            call!(e, preventDefault);
            call!(head, scrollIntoView, crate::js::obj! { behavior: "smooth", block: "start" });
            JsValue::UNDEFINED
        }));
        call!(links, appendChild, a);
        if i + 1 < n {
            let sep = call!(doc, createElement, "span");
            set(&sep, "className", "setting-contents-sep");
            set(&sep, "textContent", " \u{b7} ");
            call!(links, appendChild, sep);
        }
    }
    call!(nav, appendChild, links);
    call!(get(el, "classList"), add, "setting-contents-layout");
}
