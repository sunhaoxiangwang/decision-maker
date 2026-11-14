//! Exact distance grammar and rational comparison (PLAN v4 §8).
//!
//! number := int_part ("." [0-9]{1,6})?
//! int_part := [0-9]{1,3} ("," [0-9]{3}){1,2} | [0-9]{1,7}
//! A distance is N / 10^f in the stated unit, compared exactly in millimetres.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DistUnit {
    Mi,
    Ft,
    M,
    Km,
}

impl DistUnit {
    /// (numerator, denominator) of millimetres per unit.
    fn mm_per_unit(self) -> (u128, u128) {
        match self {
            DistUnit::Mi => (1_609_344, 1),
            DistUnit::Ft => (3_048, 10),
            DistUnit::M => (1_000, 1),
            DistUnit::Km => (1_000_000, 1),
        }
    }
    pub fn words(self) -> &'static [&'static str] {
        match self {
            DistUnit::Mi => &["mi", "mile", "miles"],
            DistUnit::Ft => &["ft", "foot", "feet"],
            DistUnit::M => &["m", "meter", "meters", "metre", "metres"],
            DistUnit::Km => &["km", "kilometer", "kilometers", "kilometre", "kilometres"],
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            DistUnit::Mi => "mi",
            DistUnit::Ft => "ft",
            DistUnit::M => "m",
            DistUnit::Km => "km",
        }
    }
}

/// Exact validated distance: the original number text and unit are kept so the exact
/// value survives restarts (rounded display values never replace it).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Distance {
    pub text: String,
    pub unit: DistUnit,
    pub numer: u64,
    pub decimals: u32,
}

impl Distance {
    pub fn parse(text: &str, unit: DistUnit) -> Option<Distance> {
        let t = text.trim();
        let (int_part, frac) = match t.split_once('.') {
            Some((i, f)) => (i, Some(f)),
            None => (t, None),
        };
        let int_digits = if int_part.contains(',') {
            let groups: Vec<&str> = int_part.split(',').collect();
            if groups.len() < 2 || groups.len() > 3 {
                return None;
            }
            if groups[0].is_empty()
                || groups[0].len() > 3
                || !groups[0].bytes().all(|b| b.is_ascii_digit())
            {
                return None;
            }
            for g in &groups[1..] {
                if g.len() != 3 || !g.bytes().all(|b| b.is_ascii_digit()) {
                    return None;
                }
            }
            groups.concat()
        } else {
            if int_part.is_empty()
                || int_part.len() > 7
                || !int_part.bytes().all(|b| b.is_ascii_digit())
            {
                return None;
            }
            int_part.to_string()
        };
        let frac_digits = match frac {
            None => String::new(),
            Some(f) if !f.is_empty() && f.len() <= 6 && f.bytes().all(|b| b.is_ascii_digit()) => {
                f.to_string()
            }
            Some(_) => return None,
        };
        let numer: u64 = format!("{int_digits}{frac_digits}").parse().ok()?;
        let d = Distance {
            text: t.to_string(),
            unit,
            numer,
            decimals: frac_digits.len() as u32,
        };
        // at most 100 miles
        let hundred_miles = Distance {
            text: "100".into(),
            unit: DistUnit::Mi,
            numer: 100,
            decimals: 0,
        };
        if d.cmp_exact(&hundred_miles) == std::cmp::Ordering::Greater {
            return None;
        }
        Some(d)
    }

    /// Millimetres as an exact fraction (num, den).
    fn mm(&self) -> (u128, u128) {
        let (pn, pd) = self.unit.mm_per_unit();
        (self.numer as u128 * pn, 10u128.pow(self.decimals) * pd)
    }

    pub fn cmp_exact(&self, other: &Distance) -> std::cmp::Ordering {
        let (a, b) = self.mm();
        let (c, d) = other.mm();
        (a * d).cmp(&(c * b))
    }

    /// Display value in thousandths of a mile, rounded up (option distances).
    pub fn milli_miles_ceil(&self) -> u128 {
        let (n, d) = self.mm();
        let num = n * 1000;
        let den = d * 1_609_344;
        num.div_ceil(den)
    }
    /// Display value in thousandths of a mile, rounded down (limits).
    pub fn milli_miles_floor(&self) -> u128 {
        let (n, d) = self.mm();
        (n * 1000) / (d * 1_609_344)
    }
    pub fn display(&self) -> String {
        format!("{} {}", self.text, self.unit.label())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cmp::Ordering::*;

    fn d(t: &str, u: DistUnit) -> Distance {
        Distance::parse(t, u).expect(t)
    }

    #[test]
    fn grammar() {
        assert!(Distance::parse("1,0004", DistUnit::Mi).is_none());
        assert!(Distance::parse("5,280", DistUnit::Ft).is_some());
        assert!(Distance::parse("5280", DistUnit::Ft).is_some());
        assert!(Distance::parse("1.0004", DistUnit::Mi).is_some());
        assert!(Distance::parse("1.", DistUnit::Mi).is_none());
        assert!(Distance::parse("1 000", DistUnit::Mi).is_none());
        assert!(Distance::parse("101", DistUnit::Mi).is_none());
        assert!(Distance::parse("1.0000001", DistUnit::Mi).is_none());
    }

    #[test]
    fn exact_boundaries() {
        let limit = d("1.0", DistUnit::Mi);
        assert_eq!(d("1.0004", DistUnit::Mi).cmp_exact(&limit), Greater);
        assert_eq!(d("1.01", DistUnit::Mi).cmp_exact(&limit), Greater);
        assert_eq!(d("5,280", DistUnit::Ft).cmp_exact(&limit), Equal);
        assert_eq!(d("1.61", DistUnit::Km).cmp_exact(&limit), Greater);
        assert_eq!(d("1.609344", DistUnit::Km).cmp_exact(&limit), Equal);
        assert_eq!(d("1", DistUnit::Mi).cmp_exact(&limit), Equal);
    }

    #[test]
    fn display_rounding() {
        assert_eq!(d("1.0004", DistUnit::Mi).milli_miles_ceil(), 1001);
        assert_eq!(d("1.0004", DistUnit::Mi).milli_miles_floor(), 1000);
        assert_eq!(d("1.61", DistUnit::Km).milli_miles_ceil(), 1001);
    }
}
