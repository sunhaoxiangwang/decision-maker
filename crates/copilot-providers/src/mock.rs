//! MOCK providers. Deterministic, rule-based stand-ins for demos and tests.
//! Every result they produce is labelled `providers: "mock"` by the server and
//! must never be presented as a working provider integration.

use crate::{BoxFut, Decider, ExtractOutput, ExtractRequest, Extractor};
use copilot_core::context::{CtxExtraction, CtxField, RawCtxChange};
use copilot_core::contextual::{ChoiceAnswer, NoulVerdict};
use copilot_core::contract::{ApiError, Mode};
use copilot_core::lunch::{
    self, LField, LPredicate, LUnit, LunchExtraction, LunchTurnKind, RawLunchFact,
};
use copilot_core::money::find_amounts;
use copilot_core::negotiation::*;
use copilot_core::text::{has_any_phrase, negated_before, tokens};
use std::collections::{BTreeMap, VecDeque};
use std::sync::Mutex;
use std::time::Duration;

pub struct MockExtractor;

fn blank(field: TermField, quote: &str) -> RawTerm {
    RawTerm {
        field,
        amount_kind: AmountKindRaw::NotApplicable,
        extra_label: ExtraLabel::NotApplicable,
        total_covers: Covers::NotApplicable,
        relation: Relation::None,
        amount_text: None,
        currency: Currency::NotApplicable,
        predicate: Predicate::NotApplicable,
        enum_value: EnumValue::NotApplicable,
        quantity_text: None,
        duration_unit: DurUnit::NotApplicable,
        refers_to: RefersTo::ThisOffer,
        corrects_earlier_value_in_turn: false,
        quote: quote.into(),
    }
}

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

pub fn mock_negotiation(transcript: &str, active_offer: Option<&str>) -> NegExtraction {
    let lower = transcript.to_lowercase();
    let mut terms = Vec::new();
    let mut saw_amount = false;
    let mut prev_amount_end: Option<usize> = None;
    for cl in clauses(transcript) {
        let toks = tokens(&cl);
        for a in find_amounts(&cl).into_iter().filter(|a| a.usd_marked) {
            let after = tokens(&cl[a.end..]);
            let before = tokens(&cl[..a.start]);
            let near_after: Vec<String> = after.iter().take(4).cloned().collect();
            let has = |w: &[&str]| has_any_phrase(&near_after, w);
            let mut t = blank(TermField::Amount, &cl);
            t.amount_text = Some(cl[a.start..a.end].to_string());
            t.currency = if a.usd_marked {
                Currency::Usd
            } else {
                Currency::Unstated
            };
            t.predicate = Predicate::Is;
            let prev = before.last().map(String::as_str).unwrap_or("");
            let first2: Vec<String> = after.iter().take(2).cloned().collect();
            let starts_incl = after
                .first()
                .is_some_and(|w| matches!(w.as_str(), "including" | "includes" | "incl"));
            if starts_incl {
                t.amount_kind = AmountKindRaw::Total;
                t.total_covers = Covers::ItemDelivery;
            } else if has_any_phrase(&first2, &["delivery", "shipping"])
                || before.ends_with(&["delivery".into(), "is".into()])
            {
                t.amount_kind = AmountKindRaw::Delivery;
                if before.iter().any(|w| w == "including" || w == "includes") {
                    t.relation = Relation::ComponentOfTotal;
                } else if prev == "plus" || prev == "+" {
                    t.relation = Relation::Separate;
                }
            } else if has_any_phrase(
                &first2,
                &[
                    "delivered",
                    "including delivery",
                    "with delivery",
                    "delivery included",
                ],
            ) {
                t.amount_kind = AmountKindRaw::Total;
                t.total_covers = Covers::ItemDelivery;
            } else if let Some(l) = [
                ("setup", ExtraLabel::Setup),
                ("assembly", ExtraLabel::Assembly),
                ("haul", ExtraLabel::HaulAway),
                ("tax", ExtraLabel::Tax),
            ]
            .iter()
            .find(|(w, _)| first2.iter().any(|x| x == w))
            .map(|(_, l)| *l)
            {
                t.amount_kind = AmountKindRaw::Extra;
                t.extra_label = l;
                t.relation = Relation::Separate;
            } else if has(&[
                "delivered",
                "including delivery",
                "with delivery",
                "delivery included",
                "including a",
                "includes",
            ]) {
                t.amount_kind = AmountKindRaw::Total;
                t.total_covers = Covers::ItemDelivery;
            } else if has(&["plus delivery", "before delivery"]) {
                t.amount_kind = AmountKindRaw::Total;
                t.total_covers = Covers::ItemOnly;
                let mut d = blank(TermField::Amount, &cl);
                d.amount_kind = AmountKindRaw::Delivery;
                terms.push(d);
            } else if has(&["out the door", "all in"]) {
                t.amount_kind = AmountKindRaw::Total;
                t.total_covers = Covers::AllIn;
            } else if before.iter().any(|w| w == "mattress" || w == "item") {
                t.amount_kind = AmountKindRaw::Item;
            } else {
                t.amount_kind = AmountKindRaw::Unspecified;
            }
            if has_any_phrase(&before, &["last week", "used to", "was"]) {
                t.refers_to = RefersTo::OtherOfferOrPast;
            }
            if let Some(pe) = prev_amount_end {
                let _ = pe;
            }
            if has_any_phrase(
                &toks,
                &["i mean", "sorry", "actually", "make that", "correction"],
            ) && saw_amount
            {
                t.corrects_earlier_value_in_turn = true;
            }
            saw_amount = true;
            prev_amount_end = Some(a.end);
            terms.push(t);
        }
        for (v, words) in [
            (EnumValue::Queen, "queen"),
            (EnumValue::King, "king"),
            (EnumValue::Twin, "twin"),
            (EnumValue::Full, "full"),
        ] {
            if let Some(p) = toks.iter().position(|t| t == words) {
                let mut t = blank(TermField::Size, &cl);
                t.enum_value = v;
                t.predicate = if negated_before(&toks, p) {
                    Predicate::IsNot
                } else {
                    Predicate::Is
                };
                terms.push(t);
            }
        }
        for (i, w) in toks.iter().enumerate() {
            let v = match w.as_str() {
                "firm" if i > 0 && toks[i - 1] == "extra" => Some(EnumValue::ExtraFirm),
                "firm" if i > 0 && toks[i - 1] == "medium" => None,
                "firm" => Some(EnumValue::Firm),
                "soft" | "plush" => Some(EnumValue::Soft),
                "medium" if toks.get(i + 1).map(String::as_str) != Some("firm") => {
                    Some(EnumValue::Medium)
                }
                _ => None,
            };
            if let Some(v) = v {
                let mut t = blank(TermField::Firmness, &cl);
                t.enum_value = v;
                t.predicate = if negated_before(&toks, i) {
                    Predicate::IsNot
                } else {
                    Predicate::Is
                };
                terms.push(t);
            }
        }
        if has_any_phrase(
            &toks,
            &[
                "no returns",
                "final sale",
                "no refunds",
                "cannot return",
                "can't return",
            ],
        ) {
            let mut t = blank(TermField::ReturnPeriod, &cl);
            t.predicate = Predicate::None;
            terms.push(t);
        } else {
            for (i, w) in toks.iter().enumerate() {
                if !w.bytes().all(|b| b.is_ascii_digit()) {
                    continue;
                }
                let unit = match toks.get(i + 1).map(String::as_str) {
                    Some("night" | "nights") => DurUnit::Nights,
                    Some("day" | "days") => DurUnit::Days,
                    Some("week" | "weeks") => DurUnit::Weeks,
                    Some("month" | "months") => DurUnit::Months,
                    Some("year" | "years") => DurUnit::Years,
                    _ => continue,
                };
                if !toks.iter().any(|t| t.starts_with("return") || t == "trial") {
                    continue;
                }
                let mut t = blank(TermField::ReturnPeriod, &cl);
                t.predicate = Predicate::Is;
                t.quantity_text = Some(w.clone());
                t.duration_unit = unit;
                terms.push(t);
            }
        }
    }
    let hypothetical = ["what if", "would you", "could you", "suppose"]
        .iter()
        .any(|m| lower.contains(m))
        || lower.trim_end().ends_with('?');
    let turn_kind = if terms.is_empty() {
        TurnKind::Irrelevant
    } else if hypothetical {
        TurnKind::QuestionOrHypothetical
    } else if active_offer.is_some() && copilot_core::session::keeps_other_terms(transcript) {
        TurnKind::Amendment
    } else if saw_amount || active_offer.is_none() {
        TurnKind::NewOffer
    } else {
        TurnKind::Amendment
    };
    NegExtraction {
        turn_kind,
        amends_offer_id: if turn_kind == TurnKind::Amendment {
            active_offer.map(String::from)
        } else {
            None
        },
        terms,
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
    fn extract_context(
        &self,
        transcript: String,
        _goals: String,
    ) -> BoxFut<'_, Result<CtxExtraction, ApiError>> {
        Box::pin(async move { Ok(mock_context(&transcript)) })
    }
    fn extract(&self, req: ExtractRequest) -> BoxFut<'_, Result<ExtractOutput, ApiError>> {
        Box::pin(async move {
            Ok(match req.mode {
                Mode::Negotiation => ExtractOutput::Negotiation(mock_negotiation(
                    &req.transcript,
                    req.active_offer_id.as_deref(),
                )),
                Mode::Lunch => {
                    ExtractOutput::Lunch(mock_lunch(&req.transcript, &req.lunch_options))
                }
            })
        })
    }
}

pub struct MockDecider;

const CONCERN_WORDS: &[&str] = &[
    "restocking",
    "only if",
    "catch",
    "non refundable",
    "extra fee",
    "additional fee",
    "surcharge",
    "condition",
    "but you have to",
];

impl Decider for MockDecider {
    fn name(&self) -> &str {
        "mock-decider"
    }
    fn noul(&self, _terms: String, window: String) -> BoxFut<'_, Result<NoulVerdict, ApiError>> {
        Box::pin(async move {
            let toks = tokens(&window);
            Ok(if has_any_phrase(&toks, CONCERN_WORDS) {
                NoulVerdict::Concern { p_yes: 0.85 }
            } else {
                NoulVerdict::Clear { p_yes: 0.05 }
            })
        })
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
    pub queue: Mutex<VecDeque<Scripted<ExtractOutput>>>,
    /// Scripted voice-context extractions (falls back to the MOCK when empty).
    pub context_queue: Mutex<VecDeque<Scripted<CtxExtraction>>>,
    pub calls: std::sync::atomic::AtomicUsize,
    pub fallback: bool,
}

impl ScriptedExtractor {
    pub fn new(items: Vec<Scripted<ExtractOutput>>) -> ScriptedExtractor {
        ScriptedExtractor {
            queue: Mutex::new(items.into()),
            context_queue: Mutex::new(VecDeque::new()),
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
    fn extract_context(
        &self,
        transcript: String,
        goals: String,
    ) -> BoxFut<'_, Result<CtxExtraction, ApiError>> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let next = self.context_queue.lock().expect("lock").pop_front();
        match next {
            Some(s) => run_script(s),
            None => MockExtractor.extract_context(transcript, goals),
        }
    }
    fn extract(&self, req: ExtractRequest) -> BoxFut<'_, Result<ExtractOutput, ApiError>> {
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
    pub nouls: Mutex<VecDeque<Scripted<NoulVerdict>>>,
    pub choices: Mutex<VecDeque<Scripted<ChoiceAnswer>>>,
    pub calls: std::sync::atomic::AtomicUsize,
}

impl ScriptedDecider {
    pub fn new(
        nouls: Vec<Scripted<NoulVerdict>>,
        choices: Vec<Scripted<ChoiceAnswer>>,
    ) -> ScriptedDecider {
        ScriptedDecider {
            nouls: Mutex::new(nouls.into()),
            choices: Mutex::new(choices.into()),
            calls: Default::default(),
        }
    }
}

impl Decider for ScriptedDecider {
    fn name(&self) -> &str {
        "scripted-mock-decider"
    }
    fn noul(&self, t: String, w: String) -> BoxFut<'_, Result<NoulVerdict, ApiError>> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        match self.nouls.lock().expect("lock").pop_front() {
            Some(s) => run_script(s),
            None => MockDecider.noul(t, w),
        }
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

/// MOCK voice-context extraction: the MOCK negotiation reader mapped onto goal
/// fields. Deterministic; never evidence of provider behaviour.
pub fn mock_context(transcript: &str) -> CtxExtraction {
    let ext = mock_negotiation(transcript, None);
    let changes = ext
        .terms
        .into_iter()
        .filter_map(|t| {
            let field = match t.field {
                TermField::Amount if t.currency == Currency::Usd && t.amount_text.is_some() => {
                    CtxField::Budget
                }
                TermField::Size if t.predicate == Predicate::Is => CtxField::Size,
                TermField::Firmness if t.predicate == Predicate::Is => CtxField::Firmness,
                TermField::ReturnPeriod if t.predicate == Predicate::Is => CtxField::ReturnPeriod,
                _ => return None,
            };
            Some(RawCtxChange {
                field,
                amount_text: t.amount_text,
                enum_value: t.enum_value,
                quantity_text: t.quantity_text,
                duration_unit: t.duration_unit,
                quote: t.quote,
            })
        })
        .collect();
    CtxExtraction { changes }
}

#[cfg(test)]
mod tests {
    use super::*;
    use copilot_core::negotiation::validate_term;

    fn all_valid(t: &str) -> Vec<Stmt> {
        let e = mock_negotiation(t, None);
        e.terms
            .iter()
            .filter_map(|r| validate_term(r, t).ok())
            .map(|v| v.stmt)
            .collect()
    }

    #[test]
    fn mock_negotiation_n_cases() {
        let s =
            all_valid("Best I can do is $1,150 delivered, queen, firm, with a 100-night return.");
        assert!(
            s.iter().any(|x| matches!(
                x,
                Stmt::Total {
                    cents: 115_000,
                    covers: Covers::ItemDelivery
                }
            )),
            "{s:?}"
        );
        assert!(
            s.iter().any(|x| matches!(
                x,
                Stmt::Return {
                    min_nights: 100,
                    ..
                }
            )),
            "{s:?}"
        );
        assert!(s.iter().any(|x| matches!(
            x,
            Stmt::Size {
                negated: false,
                value: EnumValue::Queen
            }
        )));
        assert!(s.iter().any(|x| matches!(
            x,
            Stmt::Firmness {
                negated: false,
                value: EnumValue::Firm
            }
        )));
        let s = all_valid("$1,399.");
        assert!(
            s.iter()
                .any(|x| matches!(x, Stmt::Unspecified { cents: 139_900 })),
            "{s:?}"
        );
        let s = all_valid("$1,150 delivered plus $75 setup.");
        assert!(
            s.iter()
                .any(|x| matches!(x, Stmt::ChargeAmount { cents: 7_500, .. })),
            "{s:?}"
        );
        let s = all_valid("It's $1,150 including a $100 delivery fee.");
        assert!(
            s.iter()
                .any(|x| matches!(x, Stmt::Total { cents: 115_000, .. })),
            "{s:?}"
        );
        assert!(
            s.iter().any(|x| matches!(
                x,
                Stmt::ChargeAmount {
                    charge: Charge::Delivery,
                    cents: 10_000,
                    ..
                }
            )),
            "{s:?}"
        );
        let s = all_valid("$1,150 delivered, but it's not firm.");
        assert!(
            s.iter()
                .any(|x| matches!(x, Stmt::Firmness { negated: true, .. })),
            "{s:?}"
        );
    }
}
