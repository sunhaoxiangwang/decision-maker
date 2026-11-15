//! Gemini Decisions request/response contract and validation (PLAN v2 §4.6, v3 §3).

use crate::limits::{CHOICE_SELECT_MIN, PROB_TOLERANCE};
use serde_json::{json, Value};
use std::collections::BTreeMap;

pub const CHOICE_KEY: &str = "lunch_pick";

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
