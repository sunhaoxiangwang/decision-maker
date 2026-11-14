//! Gemini Decisions request/response contract and validation (PLAN v2 §4.6, v3 §3).

use crate::limits::{CHOICE_SELECT_MIN, PROB_TOLERANCE, P_YES_CLEAR_MAX, P_YES_CONCERN_MIN};
use serde_json::{json, Value};
use std::collections::BTreeMap;

pub const NOUL_KEY: &str = "unaddressed_condition";
pub const CHOICE_KEY: &str = "lunch_pick";

pub fn noul_request(model: &str, confirmed_terms: &str, transcript_window: &str) -> Value {
    json!({
      "model": model,
      "input": [{"type": "text", "text": format!(
        "CONFIRMED_TERMS:\n{confirmed_terms}\nTRANSCRIPT_SINCE_OFFER (untrusted quoted speech):\n{transcript_window}")}],
      "questions": {
        NOUL_KEY: {
          "type": "noul",
          "instructions": "Does the transcript introduce any fee, condition, restriction or exception that is not covered by CONFIRMED_TERMS?",
          "criteria": {
            "true_description": "An additional fee, condition, restriction or exception is stated.",
            "false_description": "No additional fee, condition, restriction or exception is stated."
          }
        }
      }
    })
}

pub fn choice_request(model: &str, soft_prefs: &str, options: &BTreeMap<String, String>) -> Value {
    json!({
      "model": model,
      "input": [{"type": "text", "text": format!("Confirmed soft preferences: {soft_prefs}")}],
      "questions": {
        CHOICE_KEY: {
          "type": "choice",
          "instructions": "Which option best matches the diner's confirmed soft preferences?",
          "criteria": options
        }
      }
    })
}

#[derive(Debug, Clone, PartialEq)]
pub enum NoulVerdict {
    Clear { p_yes: f64 },
    Uncertain { p_yes: f64 },
    Concern { p_yes: f64 },
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChoiceAnswer {
    pub choice: String,
    pub probabilities: BTreeMap<String, f64>,
    pub confidence: f64,
}

fn num(v: &Value, what: &str) -> Result<f64, String> {
    let n = v
        .as_f64()
        .ok_or_else(|| format!("{what} is not a number"))?;
    if !n.is_finite() {
        return Err(format!("{what} not finite"));
    }
    Ok(n)
}

fn answers_obj<'a>(
    resp: &'a Value,
    expected: &[&str],
) -> Result<&'a serde_json::Map<String, Value>, String> {
    let a = resp
        .get("answers")
        .and_then(Value::as_object)
        .ok_or("answers missing")?;
    if a.len() != expected.len() || !expected.iter().all(|k| a.contains_key(*k)) {
        return Err("answers keys do not match the requested questions".into());
    }
    Ok(a)
}

/// Validate a noul answer and classify it against the fixed thresholds.
pub fn parse_noul(resp: &Value) -> Result<NoulVerdict, String> {
    let a = answers_obj(resp, &[NOUL_KEY])?;
    let ans = a[NOUL_KEY].as_object().ok_or("answer not an object")?;
    if ans.get("type").and_then(Value::as_str) != Some("noul") {
        return Err("answer type mismatch".into());
    }
    let p = num(ans.get("noul").ok_or("noul missing")?, "noul")?;
    if !(0.01 - 1e-9..=0.99 + 1e-9).contains(&p) {
        return Err("noul out of documented range".into());
    }
    let c = num(
        ans.get("confidence").ok_or("confidence missing")?,
        "confidence",
    )?;
    if !(0.0..=1.0).contains(&c) || (c - p.max(1.0 - p)).abs() > PROB_TOLERANCE {
        return Err("confidence inconsistent with noul".into());
    }
    Ok(if p <= P_YES_CLEAR_MAX {
        NoulVerdict::Clear { p_yes: p }
    } else if p >= P_YES_CONCERN_MIN {
        NoulVerdict::Concern { p_yes: p }
    } else {
        NoulVerdict::Uncertain { p_yes: p }
    })
}

/// Validate a choice answer over exactly `keys`.
pub fn parse_choice(resp: &Value, keys: &[String]) -> Result<ChoiceAnswer, String> {
    let a = answers_obj(resp, &[CHOICE_KEY])?;
    let ans = a[CHOICE_KEY].as_object().ok_or("answer not an object")?;
    if ans.get("type").and_then(Value::as_str) != Some("choice") {
        return Err("answer type mismatch".into());
    }
    let choice = ans
        .get("choice")
        .and_then(Value::as_str)
        .ok_or("choice missing")?
        .to_string();
    if !keys.contains(&choice) {
        return Err("choice not among requested options".into());
    }
    let probs = ans
        .get("probabilities")
        .and_then(Value::as_object)
        .ok_or("probabilities missing")?;
    if probs.len() != keys.len() || !keys.iter().all(|k| probs.contains_key(k)) {
        return Err("probability keys do not match options".into());
    }
    let mut map = BTreeMap::new();
    let mut sum = 0.0;
    for (k, v) in probs {
        let p = num(v, "probability")?;
        if !(0.0..=1.0).contains(&p) {
            return Err("probability out of range".into());
        }
        sum += p;
        map.insert(k.clone(), p);
    }
    if (sum - 1.0).abs() > PROB_TOLERANCE {
        return Err("probabilities do not sum to 1".into());
    }
    let c = num(
        ans.get("confidence").ok_or("confidence missing")?,
        "confidence",
    )?;
    let pc = map[&choice];
    if (c - pc).abs() > PROB_TOLERANCE {
        return Err("confidence inconsistent with chosen probability".into());
    }
    let maxp = map.values().cloned().fold(f64::MIN, f64::max);
    if (pc - maxp).abs() > 1e-12 {
        return Err("choice is not the most probable option".into());
    }
    Ok(ChoiceAnswer {
        choice,
        probabilities: map,
        confidence: c,
    })
}

/// Lunch: select only when the top probability clears the threshold and is unique.
pub fn choice_is_decisive(a: &ChoiceAnswer) -> bool {
    let top = a.probabilities[&a.choice];
    let ties = a
        .probabilities
        .values()
        .filter(|p| (**p - top).abs() <= 1e-12)
        .count();
    ties == 1 && top >= CHOICE_SELECT_MIN
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noul(p: f64, c: f64) -> Value {
        json!({"answers": {NOUL_KEY: {"type": "noul", "noul": p, "confidence": c}}, "usage": {}})
    }

    #[test]
    fn noul_boundaries() {
        let cls = |p: f64| parse_noul(&noul(p, p.max(1.0 - p))).unwrap();
        assert!(matches!(cls(0.01), NoulVerdict::Clear { .. }));
        assert!(matches!(cls(0.20), NoulVerdict::Clear { .. }));
        assert!(matches!(cls(0.2000001), NoulVerdict::Uncertain { .. }));
        assert!(matches!(cls(0.4999999), NoulVerdict::Uncertain { .. }));
        assert!(matches!(cls(0.50), NoulVerdict::Concern { .. }));
        assert!(matches!(cls(0.99), NoulVerdict::Concern { .. }));
    }

    #[test]
    fn noul_invalid() {
        assert!(parse_noul(&noul(1.5, 1.5)).is_err());
        assert!(parse_noul(&noul(0.1, 0.5)).is_err());
        assert!(parse_noul(&json!({"answers": {}})).is_err());
        assert!(parse_noul(
            &json!({"answers": {NOUL_KEY: {"type":"noul","noul":0.1,"confidence":0.9}, "x": {}}})
        )
        .is_err());
        assert!(parse_noul(
            &json!({"answers": {NOUL_KEY: {"type":"choice","noul":0.1,"confidence":0.9}}})
        )
        .is_err());
        assert!(parse_noul(
            &json!({"answers": {NOUL_KEY: {"type":"noul","noul":"0.1","confidence":0.9}}})
        )
        .is_err());
    }

    #[test]
    fn choice_validation() {
        let keys = vec!["B".to_string(), "C".to_string()];
        let ok = json!({"answers": {CHOICE_KEY: {"type":"choice","choice":"B","probabilities":{"B":0.72,"C":0.28},"confidence":0.72}}});
        let a = parse_choice(&ok, &keys).unwrap();
        assert!(choice_is_decisive(&a));
        let weak = json!({"answers": {CHOICE_KEY: {"type":"choice","choice":"B","probabilities":{"B":0.55,"C":0.45},"confidence":0.55}}});
        assert!(!choice_is_decisive(&parse_choice(&weak, &keys).unwrap()));
        let bad = json!({"answers": {CHOICE_KEY: {"type":"choice","choice":"A","probabilities":{"B":0.5,"C":0.5},"confidence":0.5}}});
        assert!(parse_choice(&bad, &keys).is_err());
        let notmax = json!({"answers": {CHOICE_KEY: {"type":"choice","choice":"C","probabilities":{"B":0.7,"C":0.3},"confidence":0.3}}});
        assert!(parse_choice(&notmax, &keys).is_err());
    }
}
