//! "Copy with highlights" on the main thread: an editor's selection (Live Preview or Source mode), the clipboard, and
//! the two ways it is asked for, an item in the editor's context menu and an editor command. The text with its
//! markers is [`crate::highlights::write_range`]'s, made wherever the plugin keeps its marks (its worker, say).
//!
//! Obsidian's Editor (<https://docs.obsidian.md/Reference/TypeScript+API/Editor>): somethingSelected, getValue,
//! getCursor, posToOffset (UTF-16 offsets, as CodeMirror's and the marks').

use wasm_bindgen::prelude::*;

use crate::js::{self, call, f0, f1, f3, get, obj, str};

/// The note's whole text and the selection [from, to) in its UTF-16 offsets; None when nothing is selected.
pub fn selection(editor: &JsValue) -> Option<(String, u32, u32)> {
    let text = str(&call!(editor, getValue));
    let at = |which: &str| call!(editor, posToOffset, call!(editor, getCursor, which)).as_f64().unwrap_or(0.0) as u32;
    let (from, to) = (at("from"), at("to"));
    (from < to).then_some((text, from, to))
}

/// Whether the editor has a selection.
pub fn has_selection(editor: &JsValue) -> bool {
    call!(editor, somethingSelected).is_truthy()
}

/// The note shown by an editor-menu's view or an editor command's info (`MarkdownView` or `MarkdownFileInfo`: its
/// `file.path`; empty if none).
pub fn path_of(info: &JsValue) -> String {
    str(&get(&get(info, "file"), "path"))
}

/// The text to the clipboard (<https://developer.mozilla.org/en-US/docs/Web/API/Clipboard/writeText>); Err with the
/// browser's message (the window had no focus, say).
pub async fn to_clipboard(text: String) -> Result<(), String> {
    let clipboard = get(&get(&js::global(), "navigator"), "clipboard");
    js::wait(call!(clipboard, writeText, text)).await.map(|_| ()).map_err(|e| str(&get(&e, "message")))
}

/// An item in a menu (the `editor-menu` event's, <https://docs.obsidian.md/Reference/TypeScript+API/Workspace/on('editor-menu')>),
/// with a title, a Lucide icon and what a click does (<https://docs.obsidian.md/Reference/TypeScript+API/Menu/addItem>).
pub fn menu_item(menu: &JsValue, title: &str, icon: &str, on_click: impl Fn() + Clone + 'static) {
    let (title, icon) = (title.to_string(), icon.to_string());
    call!(menu, addItem, f1(move |item| {
        let f = on_click.clone();
        call!(call!(item, setTitle, title.clone()), setIcon, icon.clone());
        call!(item, onClick, f0(f))
    }));
}

/// An editor command, listed while `check(editor, info, false)` would act: `check` is called with `checking` true to
/// ask, and false to act (<https://docs.obsidian.md/Reference/TypeScript+API/Command/editorCheckCallback>).
pub fn editor_command(plugin: &JsValue, id: &str, name: &str, mut check: impl FnMut(&JsValue, &JsValue, bool) -> bool + 'static) {
    let f = f3(move |checking, editor, info| check(&editor, &info, checking.is_truthy()).into());
    call!(plugin, addCommand, obj! { id: id, name: name, editorCheckCallback: f });
}
