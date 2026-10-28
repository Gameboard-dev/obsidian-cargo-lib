//! The question a plugin asks when the GPU has too little free memory for the models it is about to load (user,
//! 2026-10-27): asked the same way by every plugin of this workspace, through this one module. The user's own plugins
//! that hold GPU memory are named (the service's claims, native-kit's hw::claims::shortage); anything else is not. The
//! CPU is never used unasked: the answer is "free" (the named services let their memory go), "cpu", or None (stop).

use wasm_bindgen::prelude::*;

/// What the service found: the memory needed and free, and what the user's other GPU services hold (their names).
#[derive(Clone, Debug, Default)]
pub struct Shortage {
    pub need_mb: u64,
    pub free_mb: u64,
    pub others_mb: u64,
    pub holders: Vec<String>,
}

/// Who asks and for what: the plugin's name, its models ("its search models"), the work ("the search"), and the two
/// buttons' words besides freeing the GPU ("Search on the CPU", "Cancel").
pub struct Asking<'a> {
    pub plugin: &'a str,
    pub models: &'a str,
    pub task: &'a str,
    pub on_cpu: &'a str,
    pub decline: &'a str,
}

/// "Augmented View", "Quarry and Augmented View", or the services in general when none was named.
pub fn holders_text(holders: &[String]) -> String {
    match holders.len() {
        0 => "Other GPU services of your plugins".into(),
        1 => holders[0].clone(),
        n => format!("{} and {}", holders[..n - 1].join(", "), holders[n - 1]),
    }
}

/// The dialog's title and text, and its two buttons (the first is the highlighted one).
pub fn wording(s: &Shortage, a: &Asking) -> (String, String, String, String) {
    let title = format!("{}: not enough GPU memory", a.plugin);
    let head = format!("{} is asking for the GPU: {} need about {} MB of GPU memory, and {} MB is free, which is insufficient for GPU (CUDA) use.", a.plugin, a.models, s.need_mb, s.free_mb);
    if s.others_mb > 0 {
        let who = holders_text(&s.holders);
        let text = format!("{head}\n\n{who} holds about {} MB of it. Stop {who} on the GPU and switch to {}? It can be started again later.\n\nOtherwise {} runs on the CPU, which is slower.", s.others_mb, a.task, a.task);
        (title, text, format!("Stop {who} and use the GPU"), a.on_cpu.to_string())
    } else {
        let text = format!("{head}\n\nThe GPU's capacity is not enough for GPU (CUDA) use now. Free GPU memory and try again, or run {} on the CPU, which is slower.", a.task);
        (title, text, a.on_cpu.to_string(), a.decline.to_string())
    }
}

/// Asks: "free", "cpu", or None.
pub async fn choose(obs: &JsValue, app: &JsValue, s: &Shortage, a: &Asking<'_>) -> Option<&'static str> {
    let (title, text, yes, no) = wording(s, a);
    let first = crate::js::ask(obs, app, &title, &text, &yes, &no).await;
    match (s.others_mb > 0, first) {
        (true, true) => Some("free"),
        (true, false) => Some("cpu"),
        (false, true) => Some("cpu"),
        (false, false) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_words() {
        assert_eq!(holders_text(&[]), "Other GPU services of your plugins");
        assert_eq!(holders_text(&["Quarry".into(), "Augmented View".into()]), "Quarry and Augmented View");
        let a = Asking { plugin: "Quarry", models: "its search models", task: "the search", on_cpu: "Search on the CPU", decline: "Cancel" };
        let held = Shortage { need_mb: 4000, free_mb: 900, others_mb: 3000, holders: vec!["Augmented View".into()] };
        let (title, text, yes, no) = wording(&held, &a);
        assert_eq!(title, "Quarry: not enough GPU memory");
        assert!(text.contains("Augmented View holds about 3000 MB") && yes == "Stop Augmented View and use the GPU" && no == "Search on the CPU");
        let none = Shortage { others_mb: 0, ..held };
        let (_, text, yes, no) = wording(&none, &a);
        assert!(text.contains("not enough for GPU (CUDA) use now") && yes == "Search on the CPU" && no == "Cancel");
    }
}
