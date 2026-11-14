//! Exact money parsing (no floating point). Amounts are integer cents.

/// Parse an amount text such as "1,150", "1150", "1,150.50", "$1,399".
/// Grouping commas must be in groups of exactly three digits.
pub fn parse_amount_text(s: &str) -> Option<u64> {
    let s = s.trim().trim_start_matches('$').trim();
    let (int_part, frac) = match s.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (s, None),
    };
    if int_part.is_empty() {
        return None;
    }
    let digits: String = if int_part.contains(',') {
        let groups: Vec<&str> = int_part.split(',').collect();
        if groups[0].is_empty()
            || groups[0].len() > 3
            || !groups[0].chars().all(|c| c.is_ascii_digit())
        {
            return None;
        }
        for g in &groups[1..] {
            if g.len() != 3 || !g.chars().all(|c| c.is_ascii_digit()) {
                return None;
            }
        }
        groups.concat()
    } else {
        if !int_part.chars().all(|c| c.is_ascii_digit()) || int_part.len() > 9 {
            return None;
        }
        int_part.to_string()
    };
    let dollars: u64 = digits.parse().ok()?;
    let cents = match frac {
        None => 0,
        Some(f) if f.len() == 2 && f.chars().all(|c| c.is_ascii_digit()) => {
            f.parse::<u64>().ok()?
        }
        Some(_) => return None,
    };
    dollars.checked_mul(100)?.checked_add(cents)
}

/// One amount found in free text with its currency evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundAmount {
    pub cents: u64,
    pub usd_marked: bool,
    pub start: usize,
    pub end: usize,
}

/// Find every number in `text` and whether it is marked as USD ("$", "USD", "dollar(s)").
pub fn find_amounts(text: &str) -> Vec<FoundAmount> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let start = i;
            let mut j = i;
            while j < bytes.len()
                && (bytes[j].is_ascii_digit() || bytes[j] == b',' || bytes[j] == b'.')
            {
                j += 1;
            }
            // trim trailing separators (sentence punctuation)
            let mut end = j;
            while end > start && (bytes[end - 1] == b',' || bytes[end - 1] == b'.') {
                end -= 1;
            }
            let raw = &text[start..end];
            if let Some(cents) = parse_amount_text(raw) {
                let before = text[..start].trim_end();
                let after = text[end..].trim_start().to_ascii_lowercase();
                let usd_marked = before.ends_with('$')
                    || before.to_ascii_lowercase().ends_with("usd")
                    || after.starts_with("dollar")
                    || after.starts_with("usd")
                    || after.starts_with("bucks");
                out.push(FoundAmount {
                    cents,
                    usd_marked,
                    start,
                    end,
                });
            }
            i = j.max(i + 1);
        } else {
            i += 1;
        }
    }
    out
}

pub fn fmt_cents(c: u64) -> String {
    let d = c / 100;
    let r = c % 100;
    let s = d.to_string();
    let mut grouped = String::new();
    for (k, ch) in s.chars().enumerate() {
        if k > 0 && (s.len() - k).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    format!("${grouped}.{r:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_exact_amounts() {
        assert_eq!(parse_amount_text("1,150"), Some(115_000));
        assert_eq!(parse_amount_text("$1,399"), Some(139_900));
        assert_eq!(parse_amount_text("1150.50"), Some(115_050));
        assert_eq!(parse_amount_text("1,0004"), None);
        assert_eq!(parse_amount_text("1.5"), None);
    }

    #[test]
    fn finds_marked_amounts() {
        let f = find_amounts("It's $1,150 delivered plus 75 dollars setup.");
        assert_eq!(f.len(), 2);
        assert!(f[0].usd_marked && f[0].cents == 115_000);
        assert!(f[1].usd_marked && f[1].cents == 7_500);
        assert_eq!(fmt_cents(122_500), "$1,225.00");
    }
}
