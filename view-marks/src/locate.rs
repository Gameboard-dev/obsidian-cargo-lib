//! Reading view shows rendered text, not the note's source, so a passage marked in the source is found in a rendered
//! block by its visible text (markup blanked, spaces collapsed). A long passage that a rendered citation or link
//! interrupts is found by its first and last few words instead. The result is a list of segments, each with the
//! classes over it as a bit mask, so overlapping classes become one wrapper with several classes ([`segments`]).

/// A block's text with runs of white space collapsed, and where each kept character sits in the original (UTF-16).
pub struct Norm {
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

impl Norm {
    /// A rendered block's text (or a passage's visible text), normalised once for any number of [`Norm::find_all`].
    pub fn new(s: &str) -> Norm {
        normalize(s)
    }

    /// Where `needle` (a passage's visible text) sits in this block: [start, end) in the block's UTF-16 offsets, the
    /// first place only, or every place when `every`.
    pub fn find_all(&self, needle: &str, every: bool) -> Vec<(u32, u32)> {
        let n = normalize(needle);
        let mut out = Vec::new();
        let mut from = 0;
        while let Some((a, e)) = locate(self, &n, from) {
            if e == 0 || e > self.chars.len() {
                break;
            }
            out.push((self.start16[a], self.end16[e - 1]));
            if !every {
                break;
            }
            from = e;
        }
        out
    }
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

/// Overlapping ranges (start, end, class) of different classes become segments carrying every class over them:
/// (start, end, mask of classes), sorted, not overlapping. Classes are 0 to 31.
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

/// [`segments`] for ranges with a strength step (start, end, class, strength 0 to 5): each segment as a
/// [`crate::Seg`], with the greatest strength of the ranges over it.
pub fn segments_with_strength(ranges: &[(u32, u32, u8, u8)]) -> Vec<crate::Seg> {
    let mut cuts: Vec<u32> = ranges.iter().flat_map(|&(a, e, _, _)| [a, e]).collect();
    cuts.sort_unstable();
    cuts.dedup();
    let mut out: Vec<crate::Seg> = Vec::new();
    for w in cuts.windows(2) {
        let (a, e) = (w[0], w[1]);
        let over = ranges.iter().filter(|&&(s, t, _, _)| s <= a && t >= e);
        let (mask, strength) = over.fold((0u32, 0u8), |(m, st), &(_, _, k, s)| (m | (1 << k), st.max(s)));
        if mask == 0 {
            continue;
        }
        match out.last_mut() {
            Some(last) if last.to == a && last.mask == mask && last.strength == strength => last.to = e,
            _ => out.push(crate::Seg { from: a, to: e, mask, strength }),
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

    #[test]
    fn overlapping_classes_become_masked_segments() {
        let block = "Plain bold part and link text end.";
        let b = Norm::new(block);
        let mut ranges = Vec::new();
        for (what, k) in [("Plain bold part and link text", 3u8), ("bold part", 0), ("part and link", 1)] {
            ranges.extend(b.find_all(what, false).into_iter().map(|(a, e)| (a, e, k)));
        }
        let m = |ks: &[u8]| ks.iter().fold(0u32, |m, k| m | (1 << k));
        assert_eq!(segments(&ranges), vec![(0, 6, m(&[3])), (6, 11, m(&[3, 0])), (11, 15, m(&[3, 0, 1])), (15, 24, m(&[3, 1])), (24, 29, m(&[3]))]);
    }

    #[test]
    fn found_despite_spacing_case_and_an_interrupting_citation() {
        let block = "Oates  calls it\ninaccurate (Oates, 2006, p. 287) to equate positivism with quantitative research.";
        let b = Norm::new(block);
        let got = b.find_all("Calls it inaccurate", false);
        assert_eq!(&block[got[0].0 as usize..got[0].1 as usize], "calls it\ninaccurate");
        let long = "Oates calls it inaccurate to equate positivism with quantitative research";
        let got = b.find_all(long, false);
        assert_eq!(&block[got[0].0 as usize..got[0].1 as usize], &block[..block.len() - 1]);
        assert!(b.find_all("not there", true).is_empty());
    }

    #[test]
    fn every_occurrence_and_utf16() {
        let b = Norm::new("😀 theory and theory again");
        assert_eq!(b.find_all("theory", true), vec![(3, 9), (14, 20)]); // the emoji is 2 units
        assert_eq!(b.find_all("theory", false), vec![(3, 9)]);
    }

    #[test]
    fn strength_is_the_greatest_over_a_segment() {
        let segs = segments_with_strength(&[(0, 10, 2, 3), (4, 6, 5, 5), (10, 12, 2, 3), (12, 14, 2, 4)]);
        let s = |from, to, mask, strength| crate::Seg { from, to, mask, strength };
        assert_eq!(segs, vec![s(0, 4, 1 << 2, 3), s(4, 6, (1 << 2) | (1 << 5), 5), s(6, 12, 1 << 2, 3), s(12, 14, 1 << 2, 4)]);
    }

    #[test]
    fn plain_words_drop_markup() {
        assert_eq!(plain_words("The **second** [[x|link]], isn't it", 4), ["the", "second", "x", "link"]);
    }
}
