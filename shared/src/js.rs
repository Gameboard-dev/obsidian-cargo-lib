//! The few things needed to drive Obsidian and CodeMirror from Rust. Their APIs are JavaScript classes reached through
//! the module objects main.js passes in, so calls are made by name (`call!`, `obj!`) instead of through bindings for the
//! whole API: a dozen lines here instead of a binding crate to maintain; the DOM is reached the same way.
//!
//! References: `Reflect` (<https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/Reflect>), js-sys
//! (<https://docs.rs/js-sys>), Rust closures passed to JavaScript
//! (<https://rustwasm.github.io/docs/wasm-bindgen/reference/passing-rust-closures-to-js.html>), promises as futures
//! (<https://docs.rs/wasm-bindgen-futures>).

use std::future::Future;

use js_sys::{Array, Function, Object, Promise, Reflect};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;

pub fn get(o: &JsValue, k: &str) -> JsValue {
    Reflect::get(o, &JsValue::from_str(k)).unwrap_or(JsValue::UNDEFINED)
}

pub fn set(o: &JsValue, k: &str, v: impl Into<JsValue>) {
    let _ = Reflect::set(o, &JsValue::from_str(k), &v.into());
}

fn array(args: &[JsValue]) -> Array {
    let a = Array::new();
    args.iter().for_each(|v| _ = a.push(v));
    a
}

/// `f.apply(this, args)`; undefined if `f` is not a function or it throws.
pub fn apply(f: &JsValue, this: &JsValue, args: &[JsValue]) -> JsValue {
    match f.dyn_ref::<Function>() {
        Some(f) => Reflect::apply::<fn() -> JsValue>(f.unchecked_ref(), this, &array(args)).unwrap_or(JsValue::UNDEFINED),
        None => JsValue::UNDEFINED,
    }
}

/// `o.method(...args)`; undefined if `o` has no such method or it throws.
pub fn invoke(o: &JsValue, method: &str, args: &[JsValue]) -> JsValue {
    apply(&get(o, method), o, args)
}

/// `new Class(...args)`.
pub fn new(class: &JsValue, args: &[JsValue]) -> JsValue {
    Reflect::construct::<fn() -> JsValue>(class.unchecked_ref(), &array(args)).unwrap_or(JsValue::UNDEFINED)
}

/// `call!(obj, method, args...)` → `obj.method(args...)`.
#[macro_export]
macro_rules! call {
    ($o:expr, $m:ident $(, $a:expr)* $(,)?) => { $crate::js::invoke(&$o, stringify!($m), &[$($crate::wasm_bindgen::JsValue::from($a)),*]) };
}

/// `obj!{ key: value, ... }` → a JavaScript object literal.
#[macro_export]
macro_rules! obj {
    ($($k:ident : $v:expr),* $(,)?) => {{
        let o = $crate::js_sys::Object::new();
        $( $crate::js::set(&o, stringify!($k), $v); )*
        $crate::wasm_bindgen::JsValue::from(o)
    }};
}
pub use crate::{call, obj};

/// An Obsidian view (`ItemView`) of type `kind`, titled `title`, with the icon `icon`, built as a JavaScript
/// `class extends ItemView` builds one. Obsidian's View constructor reads getViewType() from the object it is making,
/// so the three methods are on its prototype before the constructor runs: given to the object afterwards, the leaf
/// opens empty (Scholar's views did, 2026-10-27). The new.target is ItemView bound (a constructor) carrying that
/// prototype, so no code is made from a string. onOpen and onClose may be set on the result.
/// (<https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/Reflect/construct>)
pub fn item_view(obs: &JsValue, leaf: JsValue, kind: &str, title: &str, icon: &str) -> JsValue {
    let class = get(obs, "ItemView");
    let proto: Object = Object::create(get(&class, "prototype").unchecked_ref());
    for (name, value) in [("getViewType", kind), ("getDisplayText", title), ("getIcon", icon)] {
        let v = value.to_string();
        set(&proto, name, Closure::<dyn Fn() -> JsValue>::new(move || JsValue::from(v.as_str())).into_js_value());
    }
    let target = class.unchecked_ref::<Function>().bind0(&JsValue::NULL);
    set(&target, "prototype", proto);
    Reflect::construct_with_new_target(class.unchecked_ref(), &Array::of1(&leaf), &target).unwrap_or_else(|_| new(&class, &[leaf]))
}

/// A Rust closure as a JavaScript function, owned by JavaScript from then on (freed when it is collected).
pub fn f0(f: impl FnMut() + 'static) -> JsValue {
    Closure::<dyn FnMut()>::new(f).into_js_value()
}
pub fn f1(f: impl FnMut(JsValue) -> JsValue + 'static) -> JsValue {
    Closure::<dyn FnMut(JsValue) -> JsValue>::new(f).into_js_value()
}
pub fn f2(f: impl FnMut(JsValue, JsValue) -> JsValue + 'static) -> JsValue {
    Closure::<dyn FnMut(JsValue, JsValue) -> JsValue>::new(f).into_js_value()
}
pub fn f3(f: impl FnMut(JsValue, JsValue, JsValue) -> JsValue + 'static) -> JsValue {
    Closure::<dyn FnMut(JsValue, JsValue, JsValue) -> JsValue>::new(f).into_js_value()
}

pub fn global() -> JsValue {
    js_sys::global().into()
}

/// Whether this page is Ignis (Obsidian in a browser, <https://ignis.thiefling.com>), which sets `window.__ignis`.
/// Ignis hands plugins look-alikes of Node's modules (`require("child_process")`, `fs`) and of
/// `FileSystemAdapter.getBasePath`, so their presence does not mean desktop: a program cannot be started from there,
/// and a path names the server's disk, not the device the page is on.
pub fn in_ignis() -> bool {
    get(&global(), "__ignis").is_object()
}

/// Whether this is desktop Obsidian, where a plugin may start programs and read this computer's disk with Node's
/// modules: Electron's `process.versions.electron` is set, and the page is not Ignis.
pub fn desktop() -> bool {
    !in_ignis() && get(&get(&get(&global(), "process"), "versions"), "electron").is_string()
}

pub fn now() -> f64 {
    call!(get(&global(), "performance"), now).as_f64().unwrap_or(0.0)
}

/// `setTimeout` on the window or the worker (<https://developer.mozilla.org/en-US/docs/Web/API/Window/setTimeout>); returns its handle.
pub fn timeout(ms: f64, f: impl FnOnce() + 'static) -> f64 {
    call!(global(), setTimeout, Closure::once_into_js(f), ms).as_f64().unwrap_or(0.0)
}

pub fn clear_timeout(handle: f64) {
    call!(global(), clearTimeout, handle);
}

/// A promise that resolves after `ms`, to await (a pause between retries).
pub async fn sleep(ms: f64) {
    let p = Promise::new(&mut |resolve, _| {
        timeout(ms, move || _ = resolve.call0(&JsValue::NULL));
    });
    _ = JsFuture::from(p).await;
}

/// `requestAnimationFrame` (<https://developer.mozilla.org/en-US/docs/Web/API/Window/requestAnimationFrame>).
pub fn frame(f: impl FnOnce() + 'static) {
    call!(global(), requestAnimationFrame, Closure::once_into_js(f));
}

/// Await a promise (or a plain value): Err with what it rejected with.
pub async fn wait(v: JsValue) -> Result<JsValue, JsValue> {
    JsFuture::from(Promise::resolve(&v)).await
}

pub fn spawn(f: impl Future<Output = ()> + 'static) {
    wasm_bindgen_futures::spawn_local(f);
}

pub fn str(v: &JsValue) -> String {
    v.as_string().unwrap_or_default()
}

pub fn empty() -> JsValue {
    Object::new().into()
}

/// A dialog (Obsidian's Modal) with a title, a message and two buttons; true when the first is chosen, false for the
/// second or when it is closed any other way.
pub async fn ask(obs: &JsValue, app: &JsValue, title: &str, message: &str, yes: &str, no: &str) -> bool {
    let modal = new(&get(obs, "Modal"), &[app.clone()]);
    call!(get(&modal, "titleEl"), setText, title);
    let content = get(&modal, "contentEl");
    let doc = get(&global(), "document");
    for para in message.split("\n\n") {
        let p = call!(doc, createElement, "p");
        set(&p, "textContent", para);
        call!(content, appendChild, p);
    }
    let row = call!(doc, createElement, "div");
    set(&row, "className", "modal-button-container");
    call!(content, appendChild, row.clone());
    let chosen = std::rc::Rc::new(std::cell::Cell::new(false));
    let promise = Promise::new(&mut |resolve, _| {
        let close = |m: &JsValue| _ = call!(m, close);
        for (label, value, cta) in [(yes, true, true), (no, false, false)] {
            let b = call!(doc, createElement, "button");
            set(&b, "textContent", label);
            if cta {
                set(&b, "className", "mod-cta");
            }
            let (m, c) = (modal.clone(), chosen.clone());
            call!(b, addEventListener, "click", f1(move |_| {
                c.set(value);
                close(&m);
                JsValue::UNDEFINED
            }));
            call!(row, appendChild, b);
        }
        let c = chosen.clone();
        set(&modal, "onClose", f0(move || {
            _ = resolve.call1(&JsValue::NULL, &JsValue::from(c.get()));
        }));
    });
    call!(modal, open);
    wait(promise.into()).await.ok().and_then(|v| v.as_bool()).unwrap_or(false)
}

/// A dialog with a button per choice (the first highlighted) and Cancel: the index chosen, None when cancelled or closed.
pub async fn choose(obs: &JsValue, app: &JsValue, title: &str, message: &str, choices: &[&str]) -> Option<usize> {
    let modal = new(&get(obs, "Modal"), &[app.clone()]);
    call!(get(&modal, "titleEl"), setText, title);
    let content = get(&modal, "contentEl");
    let doc = get(&global(), "document");
    for para in message.split("\n\n") {
        let p = call!(doc, createElement, "p");
        set(&p, "textContent", para);
        call!(content, appendChild, p);
    }
    let row = call!(doc, createElement, "div");
    set(&row, "className", "modal-button-container");
    call!(content, appendChild, row.clone());
    let chosen = std::rc::Rc::new(std::cell::Cell::new(-1i32));
    let promise = Promise::new(&mut |resolve, _| {
        for (i, label) in choices.iter().chain(["Cancel"].iter()).enumerate() {
            let b = call!(doc, createElement, "button");
            set(&b, "textContent", *label);
            if i == 0 {
                set(&b, "className", "mod-cta");
            }
            let value = if i < choices.len() { i as i32 } else { -1 };
            let (m, c) = (modal.clone(), chosen.clone());
            call!(b, addEventListener, "click", f1(move |_| {
                c.set(value);
                _ = call!(m, close);
                JsValue::UNDEFINED
            }));
            call!(row, appendChild, b);
        }
        let c = chosen.clone();
        set(&modal, "onClose", f0(move || {
            _ = resolve.call1(&JsValue::NULL, &JsValue::from(c.get()));
        }));
    });
    call!(modal, open);
    let v = wait(promise.into()).await.ok().and_then(|v| v.as_f64()).unwrap_or(-1.0);
    (v >= 0.0).then_some(v as usize)
}

/// A dialog asking for a line of text, with OK and Cancel; None when cancelled or closed.
pub async fn ask_text(obs: &JsValue, app: &JsValue, title: &str, message: &str, placeholder: &str, ok: &str) -> Option<String> {
    let modal = new(&get(obs, "Modal"), &[app.clone()]);
    call!(get(&modal, "titleEl"), setText, title);
    let content = get(&modal, "contentEl");
    let doc = get(&global(), "document");
    let p = call!(doc, createElement, "p");
    set(&p, "textContent", message);
    call!(content, appendChild, p);
    let input = call!(doc, createElement, "input");
    set(&input, "type", "text");
    set(&input, "placeholder", placeholder);
    set(&get(&input, "style"), "width", "100%");
    call!(content, appendChild, input.clone());
    let row = call!(doc, createElement, "div");
    set(&row, "className", "modal-button-container");
    call!(content, appendChild, row.clone());
    let answer: std::rc::Rc<std::cell::RefCell<Option<String>>> = Default::default();
    let promise = Promise::new(&mut |resolve, _| {
        for (label, take, cta) in [(ok, true, true), ("Cancel", false, false)] {
            let b = call!(doc, createElement, "button");
            set(&b, "textContent", label);
            if cta {
                set(&b, "className", "mod-cta");
            }
            let (m, a, i) = (modal.clone(), answer.clone(), input.clone());
            call!(b, addEventListener, "click", f1(move |_| {
                if take {
                    *a.borrow_mut() = Some(str(&get(&i, "value")));
                }
                _ = call!(m, close);
                JsValue::UNDEFINED
            }));
            call!(row, appendChild, b);
        }
        set(&modal, "onClose", f0(move || {
            _ = resolve.call0(&JsValue::NULL);
        }));
    });
    call!(modal, open);
    _ = call!(input, focus);
    _ = wait(promise.into()).await;
    let got = answer.borrow().clone();
    got
}
