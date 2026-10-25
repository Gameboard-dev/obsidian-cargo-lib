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
