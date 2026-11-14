//! Text helpers: normalisation, token phrases, quote grounding and bounded excerpts.

use crate::limits::EXCERPT_MAX_BYTES;

/// Lower-case, map typographic apostrophes to ASCII, turn hyphens and most
/// punctuation into spaces and collapse whitespace. `$`, `.`, `,` and `'` are kept
/// so amounts and contractions survive.
pub fn normalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_space = true;
    for ch in s.chars() {
        let c = match ch {
            '\u{2019}' | '\u{2018}' | '`' => '\'',
            c => c,
        };
        let keep =
            c.is_alphanumeric() || matches!(c, '$' | '.' | ',' | '\'' | '+' | '%' | '/' | ':');
        if keep {
            for lc in c.to_lowercase() {
                out.push(lc);
            }
            last_space = false;
        } else if !last_space {
            out.push(' ');
            last_space = true;
        }
    }
    while out.ends_with(' ') {
        out.pop();
    }
    out
}

/// Tokens of the normalised text with trailing sentence punctuation removed.
pub fn tokens(s: &str) -> Vec<String> {
    normalize(s)
        .split(' ')
        .map(|t| t.trim_end_matches(['.', ',', ':']).to_string())
        .filter(|t| !t.is_empty())
        .collect()
}

/// True when `phrase` (a space-separated token sequence) occurs in `toks`.
pub fn has_phrase(toks: &[String], phrase: &str) -> bool {
    let p: Vec<&str> = phrase.split(' ').collect();
    if p.is_empty() || p.len() > toks.len() {
        return false;
    }
    toks.windows(p.len())
        .any(|w| w.iter().zip(&p).all(|(a, b)| a == b))
}

/// Index of every occurrence of a phrase (start token index).
pub fn phrase_positions(toks: &[String], phrase: &str) -> Vec<usize> {
    let p: Vec<&str> = phrase.split(' ').collect();
    if p.is_empty() || p.len() > toks.len() {
        return Vec::new();
    }
    toks.windows(p.len())
        .enumerate()
        .filter(|(_, w)| w.iter().zip(&p).all(|(a, b)| a == b))
        .map(|(i, _)| i)
        .collect()
}

pub fn has_any_phrase(toks: &[String], phrases: &[&str]) -> bool {
    phrases.iter().any(|p| has_phrase(toks, p))
}

/// Negation cue tokens used by explicitly supported negative predicates.
pub fn is_negation_token(t: &str) -> bool {
    matches!(t, "not" | "no" | "never" | "isnt" | "arent" | "without") || t.ends_with("n't")
}

/// "not", "isn't", "is not", "n't" within 3 tokens before `word` (optionally "a"/"an" between).
pub fn negated_before(toks: &[String], word_idx: usize) -> bool {
    let start = word_idx.saturating_sub(3);
    toks[start..word_idx].iter().any(|t| is_negation_token(t))
}

/// Quote grounding: the quote must occur in the full transcript after normalisation.
pub fn quote_in_transcript(quote: &str, transcript: &str) -> bool {
    let q = normalize(quote);
    if q.is_empty() {
        return false;
    }
    normalize(transcript).contains(&q)
}

/// Byte range of the quote in the original transcript when it occurs verbatim
/// (ASCII case-insensitive). `None` when only the normalised form matched.
pub fn quote_byte_range(quote: &str, transcript: &str) -> Option<(usize, usize)> {
    let q = quote.trim();
    if q.is_empty() || q.len() > transcript.len() {
        return None;
    }
    let tl = transcript.to_ascii_lowercase();
    let ql = q.to_ascii_lowercase();
    tl.find(&ql).map(|s| (s, s + ql.len()))
}

/// Bounded excerpt cut on a char boundary; returns (excerpt, truncated).
pub fn excerpt(s: &str) -> (String, bool) {
    bounded(s, EXCERPT_MAX_BYTES)
}

pub fn bounded(s: &str, max_bytes: usize) -> (String, bool) {
    if s.len() <= max_bytes {
        return (s.to_string(), false);
    }
    let mut end = max_bytes.saturating_sub(3);
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = s[..end].to_string();
    out.push_str("...");
    (out, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_and_tokenizes() {
        let t = tokens("It's NOT firm—it’s peanut-free, $1,150 delivered.");
        assert!(t.contains(&"peanut".to_string()));
        assert!(t.contains(&"free".to_string()));
        assert!(t.contains(&"$1,150".to_string()));
        assert!(has_phrase(&t, "not firm"));
    }

    #[test]
    fn grounding() {
        assert!(quote_in_transcript(
            "$1,150  delivered",
            "Best is $1,150 delivered, ok"
        ));
        assert!(!quote_in_transcript(
            "$1,050 delivered",
            "Best is $1,150 delivered"
        ));
        assert!(!quote_in_transcript("   ", "x"));
    }

    #[test]
    fn excerpt_is_char_safe() {
        let s = "🙂".repeat(100); // 400 bytes
        let (e, t) = excerpt(&s);
        assert!(t);
        assert!(e.len() <= EXCERPT_MAX_BYTES);
    }
}

/// Split raw text into clauses on sentence punctuation (and on commas that are not
/// digit separators when `commas` is true).
fn split_clauses(t: &str, commas: bool) -> Vec<String> {
    let chars: Vec<char> = t.chars().collect();
    let mut out = Vec::new();
    let mut cur = String::new();
    for (i, &c) in chars.iter().enumerate() {
        let next_digit = chars.get(i + 1).is_some_and(|n| n.is_ascii_digit());
        let prev_digit = i > 0 && chars[i - 1].is_ascii_digit();
        let sentence =
            matches!(c, ';' | '!' | '?' | '\n') || (c == '.' && !(prev_digit && next_digit));
        let comma = commas && c == ',' && !(prev_digit && next_digit);
        if sentence || comma {
            if !cur.trim().is_empty() {
                out.push(cur.trim().to_string());
            }
            cur.clear();
        } else {
            cur.push(c);
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

/// The original clauses of the transcript that contain the quote: comma clauses
/// first, then whole sentences, then the whole transcript. Empty when the quote is
/// not in the transcript at all.
pub fn support_clauses(quote: &str, transcript: &str) -> Vec<String> {
    let q = normalize(quote);
    if q.is_empty() {
        return Vec::new();
    }
    for commas in [true, false] {
        let v: Vec<String> = split_clauses(transcript, commas)
            .into_iter()
            .filter(|c| normalize(c).contains(&q))
            .collect();
        if !v.is_empty() {
            return v;
        }
    }
    if normalize(transcript).contains(&q) {
        vec![transcript.to_string()]
    } else {
        Vec::new()
    }
}

#[cfg(test)]
mod clause_tests {
    use super::*;

    #[test]
    fn clauses_keep_digit_commas() {
        let c = support_clauses("firm", "It is $1,150 delivered, but it is not firm.");
        assert_eq!(c, vec!["but it is not firm".to_string()]);
        let c = support_clauses("queen, firm", "Queen, firm, 100-night return.");
        assert_eq!(c, vec!["Queen, firm, 100-night return".to_string()]);
        assert!(support_clauses("absent", "nothing here").is_empty());
    }
}
