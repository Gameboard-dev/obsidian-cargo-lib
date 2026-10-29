//! The plugin's stylesheet: one `<style>` element in the document's head, written again whenever the plugin's colours
//! change (so a colour or an opacity never asks for the marks again), and the pieces of its rules: a scope (the
//! attribute [`crate::Config::attr`] the editors and Reading views carry), a tint drawn as a background colour or over
//! one, light and dark themes, and the strength steps.
//!
//! A mark's colour is a tint, never bold, italic or underline, so the text keeps the writer's own emphasis. Light
//! themes use the colour as given; dark themes a deeper version of the same hue ([`dark`]), so light text stays
//! readable. Obsidian marks a dark theme with the class `theme-dark` on the body.

use crate::js::{self, call, get, set};

/// `#RRGGBB` or `#RGB`.
pub fn rgb(hex: &str) -> Option<(u8, u8, u8)> {
    let h = hex.trim().trim_start_matches('#');
    let v = |s: &str| u8::from_str_radix(s, 16).ok();
    match h.len() {
        6 => Some((v(&h[0..2])?, v(&h[2..4])?, v(&h[4..6])?)),
        3 => Some((v(&h[0..1])? * 17, v(&h[1..2])? * 17, v(&h[2..3])? * 17)),
        _ => None,
    }
}

/// The dark-theme version of a colour: the same hue, saturation at least 55%, lightness 32%
/// (<https://en.wikipedia.org/wiki/HSL_and_HSV#From_RGB>).
pub fn dark((r, g, b): (u8, u8, u8)) -> (u8, u8, u8) {
    let (r, g, b) = (r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let (l, d) = ((max + min) / 2.0, max - min);
    let s = if d == 0.0 { 0.0 } else { d / (1.0 - (2.0 * l - 1.0).abs()) };
    let h = if d == 0.0 {
        0.0
    } else if max == r {
        60.0 * ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    let (s, l) = (if d == 0.0 { 0.0 } else { s.max(0.55) }, 0.32);
    let c = (1.0 - (2.0 * l - 1.0f32).abs()) * s;
    let x = c * (1.0 - ((h / 60.0).rem_euclid(2.0) - 1.0).abs());
    let (r, g, b) = match (h / 60.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    let to = |v: f32| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    (to(r), to(g), to(b))
}

/// How a tint is drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Fill {
    /// A pale `background-color`: a sentence or clause.
    Background,
    /// A `background-image` drawn over the background, so a phrase inside a coloured sentence still shows.
    Overlay,
}

/// A tint's declaration, opacity `a` (0 to 1).
pub fn paint(fill: Fill, (r, g, b): (u8, u8, u8), a: f32) -> String {
    match fill {
        Fill::Background => format!("background-color: rgba({r}, {g}, {b}, {a:.2});"),
        Fill::Overlay => format!("background-image: linear-gradient(rgba({r}, {g}, {b}, {a:.2}), rgba({r}, {g}, {b}, {a:.2}));"),
    }
}

/// The selector of the editors and Reading views whose [`crate::Config::attr`] is `value`.
pub fn scope(attr: &str, value: &str) -> String {
    let v: String = value.chars().flat_map(|c| if c == '"' || c == '\\' { vec!['\\', c] } else { vec![c] }).filter(|c| !c.is_control()).collect();
    format!("[{attr}=\"{v}\"]")
}

/// A class's rules in a scope: `light` in light themes, `dark` in dark ones (declarations, from [`paint`]).
pub fn rule(scope: &str, class: &str, light: &str, dark: &str) -> String {
    format!("{scope} .{class} {{ {light} }}\n.theme-dark {scope} .{class} {{ {dark} }}\n")
}

/// A class's tint at each strength step (`{base}-s1` … `{base}-s5`, [`crate::Mark::strength`]): the opacity rises
/// from a fifth of `opacity` at step 1 to all of it at step 5. Marks with no step take the class's own rule ([`rule`]).
pub fn stepped(scope: &str, base: &str, class: &str, fill: Fill, light: (u8, u8, u8), deep: (u8, u8, u8), opacity: f32) -> String {
    (1..=crate::MAX_STRENGTH)
        .map(|n| {
            let a = opacity.clamp(0.0, 1.0) * n as f32 / crate::MAX_STRENGTH as f32;
            rule(scope, &format!("{class}.{base}-s{n}"), &paint(fill, light, a), &paint(fill, deep, a))
        })
        .collect()
}

/// The stylesheet `<style id="{id}">` in the document's head, made if it is not there, with `css` as its text.
pub fn set_stylesheet(id: &str, css: &str) {
    let doc = get(&js::global(), "document");
    let mut style = call!(doc, getElementById, id);
    if style.is_null() {
        style = call!(doc, createElement, "style");
        set(&style, "id", id);
        call!(get(&doc, "head"), appendChild, style.clone());
    }
    set(&style, "textContent", css);
}

/// Remove the stylesheet (the plugin's `onunload`).
pub fn remove_stylesheet(id: &str) {
    let style = call!(get(&js::global(), "document"), getElementById, id);
    if !style.is_null() {
        call!(style, remove);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_and_dark() {
        assert_eq!(rgb("#FFD2D2"), Some((255, 210, 210)));
        assert_eq!(rgb("abc"), Some((170, 187, 204)));
        assert_eq!(rgb("#12"), None);
        assert_eq!(dark((200, 200, 200)), (82, 82, 82), "a grey stays grey");
    }

    #[test]
    fn rules_scoped_for_both_themes() {
        let s = scope("data-x-code", "a\"b");
        assert_eq!(s, "[data-x-code=\"a\\\"b\"]");
        let css = rule(&s, "x-c0", &paint(Fill::Background, (1, 2, 3), 0.5), &paint(Fill::Overlay, (4, 5, 6), 0.25));
        assert_eq!(css, format!("{s} .x-c0 {{ background-color: rgba(1, 2, 3, 0.50); }}\n.theme-dark {s} .x-c0 {{ background-image: linear-gradient(rgba(4, 5, 6, 0.25), rgba(4, 5, 6, 0.25)); }}\n"));
    }

    #[test]
    fn five_steps_up_to_the_opacity() {
        let css = stepped("[d=\"a\"]", "x", "x-c1", Fill::Background, (10, 20, 30), (1, 2, 3), 0.5);
        assert_eq!(css.lines().count(), 10);
        assert!(css.contains("[d=\"a\"] .x-c1.x-s1 { background-color: rgba(10, 20, 30, 0.10); }"));
        assert!(css.contains(".theme-dark [d=\"a\"] .x-c1.x-s5 { background-color: rgba(1, 2, 3, 0.50); }"));
    }
}
