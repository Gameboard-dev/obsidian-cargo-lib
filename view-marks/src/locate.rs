//! Reading view shows rendered text, not the note's source, so the chosen spans are found in a rendered block by their
//! visible text (the server's `v`: markup blanked, spaces collapsed). A long span that a rendered citation or link
//! interrupts is found by its first and last few words instead. The result is a list of segments, each with the kinds
//! over it as a bit mask, so overlapping kinds become one wrapper with several classes.

use crate::pick::Span;

/// A block's text with runs of white space collapsed, and where each kept character sits in the original (UTF-16).
struct Norm {
    chars: Vec<char>,
    lower: Vec<char>,
    start16: Vec<u32>,
    end16: Vec<u32>,
}

fn normalize(s: &str) -> Norm {
    let mut n = Norm { chars: Vec::new(), lower: Vec::new(), start16: Vec::new(), end16: Vec::new() };
    let mut pos = 0u32;
    let mut space = true; // drop leading white space
    for c in s.chars() {
        let w = c.len_utf16() as u32;
        if c.is_whitespace() {
            if !space {
                n.chars.push(' ');
                n.lower.push(' ');
                n.start16.push(pos);
                n.end16.push(pos + w);
            }
            space = true;
        } else {
            n.chars.push(c);
            n.lower.push(c.to_lowercase().next().unwrap_or(c));
            n.start16.push(pos);
            n.end16.push(pos + w);
            space = false;
        }
        pos += w;
    }
    while n.chars.last() == Some(&' ') {
        n.chars.pop();
        n.lower.pop();
        n.start16.pop();
        n.end16.pop();
    }
    n
}

fn find(hay: &[char], needle: &[char], from: usize) -> Option<usize> {
    if needle.is_empty() || needle.len() > hay.len() {
        return None;
    }
    (from..=hay.len() - needle.len()).find(|&i| hay[i..i + needle.len()] == *needle)
}

/// Where `needle` sits in the block, from character `from`: exact, then ignoring case, then by its first and last
/// three words (for long spans). Returns the [start, end) character range.
fn locate(block: &Norm, needle: &Norm, from: usize) -> Option<(usize, usize)> {
    if let Some(i) = find(&block.chars, &needle.chars, from) {
        return Some((i, i + needle.chars.len()));
    }
    if let Some(i) = find(&block.lower, &needle.lower, from) {
        return Some((i, i + needle.chars.len()));
    }
    let words: Vec<&[char]> = needle.lower.split(|&c| c == ' ').filter(|w| !w.is_empty()).collect();
    if words.len() < 6 {
        return None;
    }
    let head: Vec<char> = words[..3].join(&' ');
    let tail: Vec<char> = words[words.len() - 3..].join(&' ');
    let a = find(&block.lower, &head, from)?;
    let b = find(&block.lower, &tail, a + head.len())?;
    let end = b + tail.len();
    (end - a <= needle.chars.len() * 2 + 40).then_some((a, end))
}

/// The segments to wrap in a rendered block: (start16, end16, mask of kinds), sorted, not overlapping. `found`: ranges
/// the worker found in the block itself, with their kinds (the user's own terms, bold::block_ranges; citations,
/// rules.rs); `rules`: the kinds that cut a
/// sentence's colour (pick::cut).
pub fn block_segments(block: &str, spans: &[&Span], chosen: &[(usize, bool)], found: &[(u32, u32, u8)], rules: u32) -> Vec<(u32, u32, u32)> {
    let b = normalize(block);
    let mut ranges: Vec<(u32, u32, u8)> = found.to_vec();
    for &(i, every) in chosen {
        let sp = spans[i];
        let n = normalize(&sp.v);
        let mut from = 0;
        while let Some((a, e)) = locate(&b, &n, from) {
            if e == 0 || e > b.chars.len() {
                break;
            }
            ranges.push((b.start16[a], b.end16[e - 1], sp.k));
            if !every {
                break;
            }
            from = e;
        }
    }
    crate::pick::tidy(&mut ranges);
    crate::pick::cut(&mut ranges, rules);
    segments(&ranges)
}

/// Overlapping ranges of different kinds become segments carrying every kind over them.
pub fn segments(ranges: &[(u32, u32, u8)]) -> Vec<(u32, u32, u32)> {
    let mut cuts: Vec<u32> = ranges.iter().flat_map(|&(a, e, _)| [a, e]).collect();
    cuts.sort_unstable();
    cuts.dedup();
    let mut out: Vec<(u32, u32, u32)> = Vec::new();
    for w in cuts.windows(2) {
        let (a, e) = (w[0], w[1]);
        let mask = ranges.iter().filter(|&&(s, t, _)| s <= a && t >= e).fold(0u32, |m, &(_, _, k)| m | (1 << k));
        if mask == 0 {
            continue;
        }
        match out.last_mut() {
            Some(last) if last.1 == a && last.2 == mask => last.1 = e,
            _ => out.push((a, e, mask)),
        }
    }
    out
}

/// Lower-case words of a text with markup characters dropped, to tell which source paragraph a rendered block shows.
pub fn plain_words(s: &str, n: usize) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric() && c != '\'')
        .filter(|w| !w.is_empty())
        .take(n)
        .map(str::to_lowercase)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pick::{MAIN, STRESS as PHRASE, TERM};

    fn sp(k: u8, v: &str) -> Span {
        Span { k, w: 1.0, at: vec![], v: v.into() }
    }

    #[test]
    fn overlapping_kinds_become_masked_segments() {
        let block = "Plain bold part and link text end.";
        let spans = [sp(MAIN, "Plain bold part and link text"), sp(TERM, "bold part"), sp(PHRASE, "part and link")];
        let got = block_segments(block, &spans.iter().collect::<Vec<_>>(), &[(0, false), (1, false), (2, false)], &[], 0);
        let m = |ks: &[u8]| ks.iter().fold(0u32, |m, k| m | (1 << k));
        assert_eq!(got, vec![(0, 6, m(&[MAIN])), (6, 11, m(&[MAIN, TERM])), (11, 15, m(&[MAIN, TERM, PHRASE])),
            (15, 24, m(&[MAIN, PHRASE])), (24, 29, m(&[MAIN]))]);
    }

    #[test]
    fn found_despite_spacing_case_and_an_interrupting_citation() {
        let block = "Oates  calls it\ninaccurate (Oates, 2006, p. 287) to equate positivism with quantitative research.";
        let got = block_segments(block, &[&sp(TERM, "Calls it inaccurate")], &[(0, false)], &[], 0);
        assert_eq!(got.len(), 1);
        assert_eq!(&block[got[0].0 as usize..got[0].1 as usize], "calls it\ninaccurate");
        let long = "Oates calls it inaccurate to equate positivism with quantitative research";
        let got = block_segments(block, &[&sp(MAIN, long)], &[(0, false)], &[], 0);
        assert_eq!(&block[got[0].0 as usize..got[0].1 as usize], &block[..block.len() - 1]);
    }

    #[test]
    fn every_occurrence_and_utf16() {
        let block = "😀 theory and theory again";
        let got = block_segments(block, &[&sp(TERM, "theory")], &[(0, true)], &[], 0);
        assert_eq!(got.iter().map(|g| (g.0, g.1)).collect::<Vec<_>>(), vec![(3, 9), (14, 20)]); // the emoji is 2 units
    }
}
