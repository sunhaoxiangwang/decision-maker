//! MOCK providers. Deterministic, rule-based stand-ins for demos and tests.
//! Every result they produce is labelled `providers: "mock"` by the server and
//! must never be presented as a working provider integration.

use crate::{BoxFut, Decider, ExtractRequest, Extractor};
use copilot_core::contextual::ChoiceAnswer;
use copilot_core::contract::{ApiError, Outcome};
use copilot_core::judge::Judgment;
use copilot_core::lunch::{
    self, LField, LPredicate, LUnit, LunchExtraction, LunchTurnKind, RawLunchFact,
};
use copilot_core::money::{find_amounts, fmt_cents};
use copilot_core::text::{has_any_phrase, tokens};
use std::collections::{BTreeMap, VecDeque};
use std::sync::Mutex;
use std::time::Duration;

pub struct MockExtractor;

/// Split into clauses on sentence punctuation and commas (keeping "1,150" intact).
fn clauses(t: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let chars: Vec<char> = t.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        let next_digit = chars.get(i + 1).is_some_and(|n| n.is_ascii_digit());
        let prev_digit = i > 0 && chars[i - 1].is_ascii_digit();
        let sep =
            matches!(c, ';' | '!' | '?') || ((c == '.' || c == ',') && !(prev_digit && next_digit));
        if sep {
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

/// MOCK judge: the first dollar amount in the goals is the budget, the last dollar amount
/// spoken is the price. Deterministic; it reads nothing else and is never evidence of
/// how the real model judges.
pub fn mock_judge(goals: &str, conversation: &str) -> Judgment {
    let usd = |t: &str| -> Vec<u64> {
        find_amounts(t)
            .into_iter()
            .filter(|a| a.usd_marked)
            .map(|a| a.cents)
            .collect()
    };
    let j = |outcome, reason: String| Judgment { outcome, reason };
    let Some(price) = usd(conversation).last().copied() else {
        return j(Outcome::None, "MOCK: no price stated yet.".into());
    };
    match usd(goals).first().copied() {
        None => j(
            Outcome::Ask,
            format!("MOCK: {} offered; no budget in the goals.", fmt_cents(price)),
        ),
        Some(b) if price > b => j(
            Outcome::Pause,
            format!("MOCK: {} is over the {} budget.", fmt_cents(price), fmt_cents(b)),
        ),
        Some(b) => j(
            Outcome::Proceed,
            format!("MOCK: {} is within the {} budget.", fmt_cents(price), fmt_cents(b)),
        ),
    }
}

pub fn mock_lunch(transcript: &str, options: &str) -> LunchExtraction {
    // options: "A = Thai Basil; B = Green Bowl; C = Burger Barn"
    let names: Vec<(String, String)> = options
        .split(';')
        .filter_map(|p| p.split_once('='))
        .map(|(l, n)| (l.trim().to_string(), n.trim().to_string()))
        .collect();
    let mut facts = Vec::new();
    for cl in clauses(transcript) {
        let low = cl.to_lowercase();
        let Some((letter, _)) = names
            .iter()
            .find(|(_, n)| !n.is_empty() && low.contains(&n.to_lowercase()))
        else {
            continue;
        };
        let toks = tokens(&cl);
        let base = |field: LField, pred: LPredicate| RawLunchFact {
            option: letter.clone(),
            field,
            predicate: pred,
            allergen: lunch::Allergen::NotApplicable,
            diet: lunch::Diet::NotApplicable,
            distance_text: None,
            distance_unit: LUnit::NotApplicable,
            time_text: None,
            quote: cl.clone(),
        };
        if low.contains("peanut") {
            let pred = if has_any_phrase(
                &toks,
                &[
                    "no peanuts",
                    "peanut free",
                    "without peanuts",
                    "doesn't use peanuts",
                    "does not use peanuts",
                    "don't use peanuts",
                ],
            ) {
                LPredicate::FreeOf
            } else if has_any_phrase(&toks, &["may contain", "shared kitchen", "traces of"]) {
                LPredicate::MayContain
            } else {
                LPredicate::Contains
            };
            facts.push(RawLunchFact {
                allergen: lunch::Allergen::Peanut,
                ..base(LField::Allergen, pred)
            });
        }
        for (i, w) in toks.iter().enumerate() {
            let unit = match toks.get(i + 1).map(String::as_str) {
                Some("mi" | "mile" | "miles") => LUnit::Mi,
                Some("ft" | "feet") => LUnit::Ft,
                Some("km" | "kilometers") => LUnit::Km,
                _ => continue,
            };
            if w.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                facts.push(RawLunchFact {
                    distance_text: Some(w.clone()),
                    distance_unit: unit,
                    ..base(LField::Distance, LPredicate::Is)
                });
            }
        }
        if let Some(p) = toks.iter().position(|t| t == "closes") {
            let rest: Vec<&String> = toks
                .iter()
                .skip(p + 1)
                .filter(|t| *t != "at")
                .take(2)
                .collect();
            if let Some(first) = rest.first() {
                let tt = match rest.get(1).map(|s| s.as_str()) {
                    Some("pm" | "am") => format!("{} {}", first, rest[1]),
                    _ => first.to_string(),
                };
                facts.push(RawLunchFact {
                    time_text: Some(tt),
                    ..base(LField::ClosesAt, LPredicate::Is)
                });
            }
        }
    }
    LunchExtraction {
        turn_kind: if facts.is_empty() {
            LunchTurnKind::Irrelevant
        } else {
            LunchTurnKind::OptionFacts
        },
        facts,
    }
}

impl Extractor for MockExtractor {
    fn name(&self) -> &str {
        "mock-extractor"
    }
    fn extract(&self, req: ExtractRequest) -> BoxFut<'_, Result<LunchExtraction, ApiError>> {
        Box::pin(async move { Ok(mock_lunch(&req.transcript, &req.lunch_options)) })
    }
    fn judge(&self, goals: String, conversation: String) -> BoxFut<'_, Result<Judgment, ApiError>> {
        Box::pin(async move { Ok(mock_judge(&goals, &conversation)) })
    }
}

pub struct MockDecider;

impl Decider for MockDecider {
    fn name(&self) -> &str {
        "mock-decider"
    }
    fn choice(
        &self,
        soft: String,
        options: BTreeMap<String, String>,
    ) -> BoxFut<'_, Result<ChoiceAnswer, ApiError>> {
        Box::pin(async move {
            let wants_veg = soft.contains("prefer vegetarian");
            let scored: Vec<(String, bool)> = options
                .iter()
                .map(|(k, v)| (k.clone(), wants_veg && v.contains("vegetarian options yes")))
                .collect();
            let winners = scored.iter().filter(|(_, s)| *s).count();
            let n = scored.len() as f64;
            let mut probs = BTreeMap::new();
            for (k, s) in &scored {
                let p = if winners == 1 {
                    if *s {
                        0.72
                    } else {
                        0.28 / (n - 1.0)
                    }
                } else {
                    1.0 / n
                };
                probs.insert(k.clone(), p);
            }
            let choice = probs
                .iter()
                .max_by(|a, b| a.1.total_cmp(b.1))
                .map(|(k, _)| k.clone())
                .unwrap_or_default();
            let confidence = probs[&choice];
            Ok(ChoiceAnswer {
                choice,
                probabilities: probs,
                confidence,
            })
        })
    }
}

// ---------------------------------------------------------------------------
// Scripted providers for deterministic tests (gates, delays, failures)
// ---------------------------------------------------------------------------

pub enum Scripted<T> {
    Ok(T),
    Err(ApiError),
    Delay(u64, Box<Scripted<T>>),
    Gate(tokio::sync::oneshot::Receiver<()>, Box<Scripted<T>>),
}

pub struct ScriptedExtractor {
    pub queue: Mutex<VecDeque<Scripted<LunchExtraction>>>,
    /// Scripted negotiation judgements (falls back to the MOCK judge when empty).
    pub judgments: Mutex<VecDeque<Scripted<Judgment>>>,
    pub calls: std::sync::atomic::AtomicUsize,
    pub fallback: bool,
}

impl ScriptedExtractor {
    pub fn new(items: Vec<Scripted<LunchExtraction>>) -> ScriptedExtractor {
        ScriptedExtractor {
            queue: Mutex::new(items.into()),
            judgments: Mutex::new(VecDeque::new()),
            calls: Default::default(),
            fallback: true,
        }
    }
}

fn run_script<T: Send + 'static>(s: Scripted<T>) -> BoxFut<'static, Result<T, ApiError>> {
    Box::pin(async move {
        let mut cur = s;
        loop {
            match cur {
                Scripted::Ok(v) => return Ok(v),
                Scripted::Err(e) => return Err(e),
                Scripted::Delay(ms, next) => {
                    tokio::time::sleep(Duration::from_millis(ms)).await;
                    cur = *next;
                }
                Scripted::Gate(rx, next) => {
                    let _ = rx.await;
                    cur = *next;
                }
            }
        }
    })
}

impl Extractor for ScriptedExtractor {
    fn name(&self) -> &str {
        "scripted-mock-extractor"
    }
    fn judge(&self, goals: String, conversation: String) -> BoxFut<'_, Result<Judgment, ApiError>> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let next = self.judgments.lock().expect("lock").pop_front();
        match next {
            Some(s) => run_script(s),
            None => MockExtractor.judge(goals, conversation),
        }
    }
    fn extract(&self, req: ExtractRequest) -> BoxFut<'_, Result<LunchExtraction, ApiError>> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let next = self.queue.lock().expect("lock").pop_front();
        match next {
            Some(s) => run_script(s),
            None if self.fallback => MockExtractor.extract(req),
            None => Box::pin(async {
                Err(ApiError::new(
                    "extraction_unavailable",
                    "script exhausted",
                    true,
                ))
            }),
        }
    }
}

pub struct ScriptedDecider {
    pub choices: Mutex<VecDeque<Scripted<ChoiceAnswer>>>,
    pub calls: std::sync::atomic::AtomicUsize,
}

impl ScriptedDecider {
    pub fn new(choices: Vec<Scripted<ChoiceAnswer>>) -> ScriptedDecider {
        ScriptedDecider {
            choices: Mutex::new(choices.into()),
            calls: Default::default(),
        }
    }
}

impl Decider for ScriptedDecider {
    fn name(&self) -> &str {
        "scripted-mock-decider"
    }
    fn choice(
        &self,
        s: String,
        o: BTreeMap<String, String>,
    ) -> BoxFut<'_, Result<ChoiceAnswer, ApiError>> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        match self.choices.lock().expect("lock").pop_front() {
            Some(sc) => run_script(sc),
            None => MockDecider.choice(s, o),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_judge_budget() {
        let g = "a firm bed for $100";
        assert_eq!(mock_judge(g, "speaker0: hello").outcome, Outcome::None);
        assert_eq!(mock_judge(g, "speaker0: $120.").outcome, Outcome::Pause);
        assert_eq!(mock_judge(g, "speaker0: $120.\nspeaker0: ok, $90.").outcome, Outcome::Proceed);
        assert_eq!(mock_judge("firm", "speaker0: $90.").outcome, Outcome::Ask);
    }
}
