//! Which paragraphs to score, for a plugin that scores a note's paragraphs (in a worker, through a model, by rule) and
//! draws marks over them: a note's text split into paragraphs ([`Doc`]), the views showing it ([`View`]), and, for
//! each view, the paragraphs it wants, nearest the screen first ([`around`]).
//!
//! - A paragraph is known by the hash of its text ([`crate::split::Para::id`]): a plugin keeps its scores by that
//!   hash, so editing one paragraph changes only its hash, and only it needs scoring again.
//! - A paragraph that has just changed waits [`Timing::quiet_ms`] before it is wanted, so typing does not score every
//!   draft.
//! - What is on screen is wanted once the view has rested [`Timing::settle_ms`] (a fast scroll asks for nothing); the
//!   paragraphs either side (`context`) once it has rested [`Timing::prefetch_ms`].

use std::collections::{HashMap, HashSet};

use crate::split::{split, Para};

/// How long a view rests, and a changed paragraph stays quiet, before paragraphs are wanted (milliseconds).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Timing {
    pub settle_ms: f64,
    pub prefetch_ms: f64,
    pub quiet_ms: f64,
}

/// A note's text and its paragraphs.
#[derive(Default)]
pub struct Doc {
    pub text: String,
    /// The version the editor sent it with; it comes back with the marks made for it.
    pub version: u32,
    pub paras: Vec<Para>,
    pub ids: HashSet<u64>,
    /// Paragraphs that appeared since the note was opened: when.
    pub born: HashMap<u64, f64>,
}

impl Doc {
    /// New text: split again; paragraphs that are new (when `fresh`: an edit, not the note just opened) are born now,
    /// and stay quiet for `quiet_ms`.
    pub fn set_text(&mut self, text: String, now: f64, fresh: bool, quiet_ms: f64) {
        self.paras = split(&text);
        let ids: HashSet<u64> = self.paras.iter().map(|p| p.id).collect();
        if fresh {
            for id in ids.difference(&self.ids) {
                self.born.insert(*id, now);
            }
        }
        self.born.retain(|id, t| ids.contains(id) && now - *t < quiet_ms);
        self.ids = ids;
        self.text = text;
    }

    /// Indices of the paragraphs overlapping [from, to] in UTF-16 offsets.
    pub fn paras_in(&self, from: u32, to: u32) -> std::ops::Range<usize> {
        let a = self.paras.partition_point(|p| p.to16 < from);
        a..self.paras.partition_point(|p| p.from16 <= to).max(a)
    }

    /// The lines a view's visible offsets cover (the nearest paragraph's when they fall between paragraphs).
    pub fn lines_of(&self, (from, to): (u32, u32)) -> (u32, u32) {
        let r = self.paras_in(from, to);
        let (a, b) = if r.is_empty() { (r.start.min(self.paras.len().saturating_sub(1)), r.start) } else { (r.start, r.end - 1) };
        match (self.paras.get(a), self.paras.get(b.min(self.paras.len().saturating_sub(1)))) {
            (Some(p), Some(q)) => (p.line_from, q.line_to),
            _ => (0, 0),
        }
    }

    pub fn para_text(&self, p: &Para) -> &str {
        &self.text[p.start..p.end]
    }

    /// Whether a paragraph changed less than `quiet_ms` ago.
    pub fn quiet(&self, id: u64, now: f64, quiet_ms: f64) -> bool {
        self.born.get(&id).is_some_and(|t| now - t < quiet_ms)
    }
}

/// A view of a note: an editor (its visible and rendered ranges, UTF-16 offsets) or a Reading view (its visible lines).
pub struct View {
    pub path: String,
    pub editor: bool,
    pub vis: (u32, u32),
    pub render: (u32, u32),
    /// When `vis` last changed.
    pub moved: f64,
}

impl View {
    /// The view as it is now: `moved` kept while it shows the same note and range, else now.
    pub fn new(old: Option<&View>, path: String, editor: bool, vis: (u32, u32), render: (u32, u32), now: f64) -> View {
        let moved = old.filter(|v| v.path == path && v.vis == vis).map_or(now, |v| v.moved);
        View { path, editor, vis, render, moved }
    }

    /// Milliseconds until it has rested long enough for the screen, then for the context (none when both passed).
    pub fn rests(&self, now: f64, t: &Timing) -> [f64; 2] {
        [self.moved + t.settle_ms - now, self.moved + t.prefetch_ms - now]
    }
}

/// The paragraphs a view wants, as (distance from the screen in paragraphs, index in `doc.paras`), in the order of the
/// paragraphs: none until it has rested `settle_ms`; then those on screen; once it has rested `prefetch_ms`, those
/// within `context` paragraphs too; never one that is still quiet after an edit. The caller drops those it has
/// scores for, or is scoring, and sorts by distance.
pub fn around<'a>(doc: &'a Doc, v: &View, now: f64, context: u32, t: &Timing) -> impl Iterator<Item = (u32, usize)> + 'a {
    let rested = now - v.moved;
    let reach = if rested < t.prefetch_ms { 0 } else { context as usize };
    let (lf, lt) = if v.editor { doc.lines_of(v.vis) } else { v.vis };
    // the paragraphs on screen, by index: from the first that reaches the view to the last that starts in it
    let first = doc.paras.partition_point(|p| p.line_to < lf);
    let last = doc.paras.partition_point(|p| p.line_from <= lt).saturating_sub(1);
    let range = if rested < t.settle_ms || doc.paras.is_empty() { 0..0 } else { first.saturating_sub(reach)..(last + reach + 1).min(doc.paras.len()) };
    let quiet = t.quiet_ms;
    range.filter_map(move |i| {
        let dist = (first.saturating_sub(i)).max(i.saturating_sub(last)) as u32;
        (!doc.quiet(doc.paras[i].id, now, quiet)).then_some((dist, i))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: Timing = Timing { settle_ms: 150.0, prefetch_ms: 1000.0, quiet_ms: 1500.0 };

    fn doc(n: usize) -> Doc {
        let mut d = Doc::default();
        d.set_text((0..n).map(|i| format!("Paragraph {i}.")).collect::<Vec<_>>().join("\n\n"), 0.0, false, T.quiet_ms);
        d
    }

    fn view(editor: bool, vis: (u32, u32), moved: f64) -> View {
        View { path: "a.md".into(), editor, vis, render: (0, u32::MAX), moved }
    }

    /// The original rule, over every paragraph: what `around` must equal.
    fn every(doc: &Doc, v: &View, now: f64, context: u32) -> Vec<(u32, usize)> {
        let rested = now - v.moved;
        if rested < T.settle_ms {
            return Vec::new();
        }
        let reach = if rested < T.prefetch_ms { 0 } else { context };
        let (lf, lt) = if v.editor { doc.lines_of(v.vis) } else { v.vis };
        let first = doc.paras.iter().position(|p| p.line_to >= lf).unwrap_or(doc.paras.len());
        let last = doc.paras.iter().rposition(|p| p.line_from <= lt).unwrap_or(0);
        let mut out = Vec::new();
        for (i, p) in doc.paras.iter().enumerate() {
            let dist = (first.saturating_sub(i)).max(i.saturating_sub(last)) as u32;
            if dist > reach || doc.quiet(p.id, now, T.quiet_ms) {
                continue;
            }
            out.push((dist, i));
        }
        out
    }

    #[test]
    fn the_screen_then_the_context_once_rested() {
        let d = doc(10);
        let v = view(false, (10, 10), 0.0); // Reading view: line 10 is paragraph 5
        assert!(around(&d, &v, 100.0, 1, &T).next().is_none(), "still moving");
        assert_eq!(around(&d, &v, 200.0, 1, &T).collect::<Vec<_>>(), [(0, 5)]);
        assert_eq!(around(&d, &v, 1000.0, 1, &T).collect::<Vec<_>>(), [(1, 4), (0, 5), (1, 6)]);
    }

    #[test]
    fn the_same_as_the_rule_over_every_paragraph() {
        let mut d = doc(30);
        let edited = d.text.replace("Paragraph 12.", "Paragraph 12 edited.");
        d.set_text(edited, 500.0, true, T.quiet_ms);
        for editor in [false, true] {
            for vis in [(0, 0), (3, 9), (20, 40), (58, 58), (59, 200), (100, 120)] {
                for (now, ctx) in [(100.0, 2), (300.0, 2), (1200.0, 0), (1200.0, 3), (1200.0, 50), (3000.0, 4)] {
                    let v = view(editor, vis, 0.0);
                    assert_eq!(around(&d, &v, now, ctx, &T).collect::<Vec<_>>(), every(&d, &v, now, ctx), "{editor} {vis:?} {now} {ctx}");
                }
            }
        }
        assert!(around(&Doc::default(), &view(true, (0, 5), 0.0), 5000.0, 3, &T).next().is_none());
    }

    #[test]
    fn a_view_keeps_when_it_moved_while_it_shows_the_same() {
        let a = View::new(None, "a.md".into(), true, (0, 10), (0, 20), 5.0);
        let b = View::new(Some(&a), "a.md".into(), true, (0, 10), (0, 30), 9.0);
        assert_eq!(b.moved, 5.0);
        let c = View::new(Some(&b), "a.md".into(), true, (5, 10), (0, 30), 12.0);
        assert_eq!(c.moved, 12.0);
        assert_eq!(c.rests(12.0, &T), [150.0, 1000.0]);
    }
}
