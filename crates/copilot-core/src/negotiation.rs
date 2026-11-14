//! Negotiation extraction contract, deterministic validation, cost accounting and
//! requirement evaluation (PLAN v3 §2, v4 §1).

use crate::money::{find_amounts, fmt_cents, parse_amount_text};
use crate::text::{
    excerpt, has_any_phrase, is_negation_token, negated_before, normalize, phrase_positions,
    quote_byte_range, quote_in_transcript, support_clauses, tokens,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

// ---------------------------------------------------------------------------
// Extraction contract (strict; unknown fields rejected)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnKind {
    NewOffer,
    Amendment,
    QuestionOrHypothetical,
    Irrelevant,
    Ambiguous,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TermField {
    Amount,
    Size,
    Firmness,
    ReturnPeriod,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AmountKindRaw {
    Item,
    Delivery,
    Extra,
    Total,
    Unspecified,
    NotApplicable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtraLabel {
    Setup,
    Assembly,
    HaulAway,
    Tax,
    Other,
    NotApplicable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Covers {
    ItemDelivery,
    ItemOnly,
    AllIn,
    NotApplicable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Relation {
    Separate,
    ComponentOfTotal,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Currency {
    #[serde(rename = "USD")]
    Usd,
    #[serde(rename = "other")]
    Other,
    #[serde(rename = "unstated")]
    Unstated,
    #[serde(rename = "not_applicable")]
    NotApplicable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Predicate {
    Is,
    IsNot,
    None,
    NotApplicable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnumValue {
    Twin,
    Full,
    Queen,
    King,
    CalKing,
    Soft,
    Medium,
    Firm,
    ExtraFirm,
    NotApplicable,
}

impl EnumValue {
    pub fn is_size(self) -> bool {
        matches!(
            self,
            EnumValue::Twin
                | EnumValue::Full
                | EnumValue::Queen
                | EnumValue::King
                | EnumValue::CalKing
        )
    }
    pub fn is_firmness(self) -> bool {
        matches!(
            self,
            EnumValue::Soft | EnumValue::Medium | EnumValue::Firm | EnumValue::ExtraFirm
        )
    }
    pub fn label(self) -> &'static str {
        match self {
            EnumValue::Twin => "twin",
            EnumValue::Full => "full",
            EnumValue::Queen => "queen",
            EnumValue::King => "king",
            EnumValue::CalKing => "cal king",
            EnumValue::Soft => "soft",
            EnumValue::Medium => "medium",
            EnumValue::Firm => "firm",
            EnumValue::ExtraFirm => "extra firm",
            EnumValue::NotApplicable => "n/a",
        }
    }
    pub fn parse_label(s: &str) -> Option<EnumValue> {
        Some(match s {
            "twin" => EnumValue::Twin,
            "full" => EnumValue::Full,
            "queen" => EnumValue::Queen,
            "king" => EnumValue::King,
            "cal_king" => EnumValue::CalKing,
            "soft" => EnumValue::Soft,
            "medium" => EnumValue::Medium,
            "firm" => EnumValue::Firm,
            "extra_firm" => EnumValue::ExtraFirm,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DurUnit {
    Nights,
    Days,
    Weeks,
    Months,
    Years,
    NotApplicable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefersTo {
    ThisOffer,
    OtherOfferOrPast,
    Hypothetical,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawTerm {
    pub field: TermField,
    pub amount_kind: AmountKindRaw,
    pub extra_label: ExtraLabel,
    pub total_covers: Covers,
    pub relation: Relation,
    pub amount_text: Option<String>,
    pub currency: Currency,
    pub predicate: Predicate,
    pub enum_value: EnumValue,
    pub quantity_text: Option<String>,
    pub duration_unit: DurUnit,
    pub refers_to: RefersTo,
    pub corrects_earlier_value_in_turn: bool,
    pub quote: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NegExtraction {
    pub turn_kind: TurnKind,
    pub amends_offer_id: Option<String>,
    pub terms: Vec<RawTerm>,
}

/// JSON schema sent to Gemini (documented subset; enums use sentinels, not null).
pub fn extraction_schema() -> serde_json::Value {
    serde_json::json!({
      "type": "object", "additionalProperties": false,
      "required": ["turn_kind", "amends_offer_id", "terms"],
      "properties": {
        "turn_kind": {"type": "string", "enum": ["new_offer","amendment","question_or_hypothetical","irrelevant","ambiguous"]},
        "amends_offer_id": {"type": ["string","null"]},
        "terms": {"type": "array", "items": {
          "type": "object", "additionalProperties": false,
          "required": ["field","amount_kind","extra_label","total_covers","relation","amount_text","currency","predicate","enum_value","quantity_text","duration_unit","refers_to","corrects_earlier_value_in_turn","quote"],
          "properties": {
            "field": {"type":"string","enum":["amount","size","firmness","return_period"]},
            "amount_kind": {"type":"string","enum":["item","delivery","extra","total","unspecified","not_applicable"]},
            "extra_label": {"type":"string","enum":["setup","assembly","haul_away","tax","other","not_applicable"]},
            "total_covers": {"type":"string","enum":["item_delivery","item_only","all_in","not_applicable"]},
            "relation": {"type":"string","enum":["separate","component_of_total","none"]},
            "amount_text": {"type":["string","null"]},
            "currency": {"type":"string","enum":["USD","other","unstated","not_applicable"]},
            "predicate": {"type":"string","enum":["is","is_not","none","not_applicable"]},
            "enum_value": {"type":"string","enum":["twin","full","queen","king","cal_king","soft","medium","firm","extra_firm","not_applicable"]},
            "quantity_text": {"type":["string","null"]},
            "duration_unit": {"type":"string","enum":["nights","days","weeks","months","years","not_applicable"]},
            "refers_to": {"type":"string","enum":["this_offer","other_offer_or_past","hypothetical"]},
            "corrects_earlier_value_in_turn": {"type":"boolean"},
            "quote": {"type":"string"}
          }
        }}
      }
    })
}

// ---------------------------------------------------------------------------
// Validated statements
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "charge", content = "label")]
pub enum Charge {
    Item,
    Delivery,
    Extra(ExtraLabel),
}

impl Charge {
    pub fn label(self) -> String {
        match self {
            Charge::Item => "item price".into(),
            Charge::Delivery => "delivery fee".into(),
            Charge::Extra(l) => format!("{} charge", format!("{l:?}").to_lowercase()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum Stmt {
    /// A charge with a stated amount.
    ChargeAmount {
        charge: Charge,
        cents: u64,
        /// Separate charges stay additional to totals; components of a total stay included.
        relation: Relation,
    },
    /// A charge mentioned without an amount ("plus delivery").
    ChargeMentioned {
        charge: Charge,
    },
    /// A stated total and what it covers.
    Total {
        cents: u64,
        covers: Covers,
    },
    /// A bare price with unknown coverage: a lower bound only.
    Unspecified {
        cents: u64,
    },
    /// A price statement whose relationship is not supported.
    PriceUnresolved {
        reason: String,
    },
    Size {
        negated: bool,
        value: EnumValue,
    },
    Firmness {
        negated: bool,
        value: EnumValue,
    },
    Return {
        min_nights: u32,
        max_nights: u32,
    },
    ReturnNone,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Slot {
    Charge(Charge),
    Total(Covers),
    Unspecified,
    PriceUnresolved,
    Size,
    Firmness,
    Return,
}

impl Stmt {
    pub fn slot(&self) -> Slot {
        match self {
            Stmt::ChargeAmount { charge, .. } | Stmt::ChargeMentioned { charge } => {
                Slot::Charge(*charge)
            }
            Stmt::Total { covers, .. } => Slot::Total(*covers),
            Stmt::Unspecified { .. } => Slot::Unspecified,
            Stmt::PriceUnresolved { .. } => Slot::PriceUnresolved,
            Stmt::Size { .. } => Slot::Size,
            Stmt::Firmness { .. } => Slot::Firmness,
            Stmt::Return { .. } | Stmt::ReturnNone => Slot::Return,
        }
    }
    pub fn describe(&self) -> String {
        match self {
            Stmt::ChargeAmount {
                charge,
                cents,
                relation,
            } => format!(
                "{} {}{}",
                charge.label(),
                fmt_cents(*cents),
                match relation {
                    Relation::Separate => " (additional)",
                    Relation::ComponentOfTotal => " (included in total)",
                    Relation::None => "",
                }
            ),
            Stmt::ChargeMentioned { charge } => format!("{} (amount not stated)", charge.label()),
            Stmt::Total { cents, covers } => format!(
                "total {} ({})",
                fmt_cents(*cents),
                match covers {
                    Covers::ItemDelivery => "incl. delivery",
                    Covers::ItemOnly => "before delivery",
                    Covers::AllIn => "all-in",
                    Covers::NotApplicable => "coverage unknown",
                }
            ),
            Stmt::Unspecified { cents } => {
                format!("price {} (coverage not stated)", fmt_cents(*cents))
            }
            Stmt::PriceUnresolved { reason } => format!("unresolved price statement: {reason}"),
            Stmt::Size { negated, value } => format!(
                "size {}{}",
                if *negated { "is not " } else { "" },
                value.label()
            ),
            Stmt::Firmness { negated, value } => {
                format!(
                    "firmness {}{}",
                    if *negated { "is not " } else { "" },
                    value.label()
                )
            }
            Stmt::Return {
                min_nights,
                max_nights,
            } if min_nights == max_nights => format!("{min_nights}-night return"),
            Stmt::Return {
                min_nights,
                max_nights,
            } => format!("{min_nights}-{max_nights}-night return"),
            Stmt::ReturnNone => "no returns".into(),
        }
    }
}

/// A validated term from one turn before it becomes a fact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedTerm {
    pub stmt: Stmt,
    pub correction: bool,
    pub quote: String,
    pub excerpt: String,
    pub excerpt_truncated: bool,
    pub byte_range: Option<(usize, usize)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RejectedTerm {
    pub reason: String,
    pub excerpt: String,
    pub field: TermField,
    /// True only for terms deliberately classified as context (another/earlier offer).
    pub context_only: bool,
}

const UNSUPPORTED_PRICE_PHRASES: &[&str] = &[
    "discount",
    "minus",
    "off",
    "%",
    "percent",
    "per month",
    "a month",
    "/month",
    "monthly",
    "financing",
    "financed",
    "rebate",
    "coupon",
    "tax not included",
    "plus tax",
    "before tax",
];

fn amount_kind_supported(
    kind: AmountKindRaw,
    covers: Covers,
    toks: &[String],
) -> Result<Option<Covers>, String> {
    // Ok(Some(covers)) for total with supported coverage; Ok(None) otherwise.
    match kind {
        AmountKindRaw::Total => {
            let negated_incl = phrase_positions(toks, "including")
                .into_iter()
                .chain(phrase_positions(toks, "included"))
                .chain(phrase_positions(toks, "includes"))
                .any(|i| negated_before(toks, i));
            let item_delivery = !negated_incl
                && !has_any_phrase(
                    toks,
                    &[
                        "plus delivery",
                        "before delivery",
                        "excluding delivery",
                        "delivery extra",
                    ],
                )
                && (has_any_phrase(toks, &["delivered", "delivery included", "with delivery"])
                    || (toks.iter().any(|t| t == "delivery")
                        && toks.iter().any(|t| {
                            matches!(t.as_str(), "including" | "includes" | "included" | "incl")
                        })));
            let item_only = has_any_phrase(
                toks,
                &[
                    "plus delivery",
                    "before delivery",
                    "excluding delivery",
                    "not including delivery",
                    "delivery extra",
                ],
            );
            let all_in = has_any_phrase(toks, &["out the door", "all in", "everything included"]);
            match covers {
                Covers::ItemDelivery if item_delivery => Ok(Some(Covers::ItemDelivery)),
                Covers::ItemOnly if item_only => Ok(Some(Covers::ItemOnly)),
                Covers::AllIn if all_in => Ok(Some(Covers::AllIn)),
                _ => Err("total coverage phrase not supported by the quote".into()),
            }
        }
        _ => Ok(None),
    }
}

fn extra_words(l: ExtraLabel) -> &'static [&'static str] {
    match l {
        ExtraLabel::Setup => &["setup", "set up"],
        ExtraLabel::Assembly => &["assembly"],
        ExtraLabel::HaulAway => &["haul away", "removal"],
        ExtraLabel::Tax => &["tax"],
        _ => &[],
    }
}

fn value_words(v: EnumValue) -> &'static [&'static str] {
    match v {
        EnumValue::Twin => &["twin"],
        EnumValue::Full => &["full", "double"],
        EnumValue::Queen => &["queen"],
        EnumValue::King => &["king"],
        EnumValue::CalKing => &["cal king", "california king"],
        EnumValue::Soft => &["soft", "plush"],
        EnumValue::Medium => &["medium"],
        EnumValue::Firm => &["firm"],
        EnumValue::ExtraFirm => &["extra firm"],
        EnumValue::NotApplicable => &[],
    }
}

/// Positions of a value word, excluding compound forms that mean something else
/// ("extra firm"/"medium firm" are not "firm"; "cal king" is not "king").
fn value_positions(toks: &[String], v: EnumValue) -> Vec<usize> {
    let mut out = Vec::new();
    for w in value_words(v) {
        for p in phrase_positions(toks, w) {
            let prev = if p > 0 { toks[p - 1].as_str() } else { "" };
            let excluded = match v {
                EnumValue::Firm => matches!(prev, "extra" | "medium"),
                EnumValue::King => matches!(prev, "cal" | "california"),
                _ => false,
            };
            if !excluded {
                out.push(p);
            }
        }
    }
    out
}

const RETURN_NONE_PHRASES: &[&str] = &[
    "no returns",
    "no return",
    "returns not accepted",
    "final sale",
    "can't return",
    "cannot return",
    "non returnable",
    "nonreturnable",
    "no refunds",
];

fn duration_supported(qty: &str, unit: DurUnit, toks: &[String]) -> bool {
    let stems: &[&str] = match unit {
        DurUnit::Nights => &["night", "nights"],
        DurUnit::Days => &["day", "days"],
        DurUnit::Weeks => &["week", "weeks"],
        DurUnit::Months => &["month", "months"],
        DurUnit::Years => &["year", "years"],
        DurUnit::NotApplicable => return false,
    };
    toks.iter()
        .enumerate()
        .any(|(i, t)| t == qty && toks.get(i + 1).is_some_and(|n| stems.contains(&n.as_str())))
}

/// Validate one raw term against the FULL transcript (before any excerpting).
pub fn validate_term(raw: &RawTerm, transcript: &str) -> Result<ValidatedTerm, RejectedTerm> {
    let (ex, trunc) = excerpt(&raw.quote);
    let reject = |reason: &str| RejectedTerm {
        reason: reason.to_string(),
        excerpt: ex.clone(),
        field: raw.field,
        context_only: false,
    };
    if !quote_in_transcript(&raw.quote, transcript) {
        return Err(reject("quote not found in transcript"));
    }
    match raw.refers_to {
        RefersTo::ThisOffer => {}
        RefersTo::OtherOfferOrPast => {
            return Err(RejectedTerm {
                context_only: true,
                ..reject("refers to another or earlier offer (context only)")
            })
        }
        RefersTo::Hypothetical => return Err(reject("hypothetical (not applied)")),
    }
    // Evaluate the claim in every clause of the FULL transcript that contains the
    // quote: negation, predicates and roles come from the original clause, not
    // from the (possibly shortened) quote. Every occurrence must agree.
    let clauses = support_clauses(&raw.quote, transcript);
    if clauses.is_empty() {
        return Err(reject("quote not found in transcript"));
    }
    let mut stmt: Option<Stmt> = None;
    for clause in &clauses {
        let s = validate_in(raw, clause).map_err(|r| {
            let (c, _) = excerpt(clause);
            reject(&format!("{r} (context: \"{c}\")"))
        })?;
        match &stmt {
            Some(prev) if prev != &s => {
                return Err(reject(
                    "the quoted words support different readings in different clauses",
                ))
            }
            _ => stmt = Some(s),
        }
    }
    let stmt = stmt.expect("at least one clause");
    Ok(ValidatedTerm {
        stmt,
        correction: raw.corrects_earlier_value_in_turn,
        quote: raw.quote.clone(),
        excerpt: ex,
        excerpt_truncated: trunc,
        byte_range: quote_byte_range(&raw.quote, transcript),
    })
}

/// Validate a claim against one supporting clause of the transcript.
fn validate_in(raw: &RawTerm, evidence: &str) -> Result<Stmt, String> {
    let toks = tokens(evidence);
    Ok(match raw.field {
        TermField::Amount => validate_amount(raw, evidence)?,
        TermField::Size | TermField::Firmness => {
            let want_size = raw.field == TermField::Size;
            let v = raw.enum_value;
            if (want_size && !v.is_size()) || (!want_size && !v.is_firmness()) {
                return Err("value does not match field".into());
            }
            let pos = value_positions(&toks, v);
            if pos.is_empty() {
                return Err("value word not in the supporting clause".into());
            }
            let negs = pos.iter().filter(|&&p| negated_before(&toks, p)).count();
            let negated = match raw.predicate {
                // affirmative only when no occurrence in the clause is negated
                Predicate::Is if negs == 0 => false,
                Predicate::Is => return Err("the clause negates the claimed value".into()),
                // negative only when every occurrence is negated
                Predicate::IsNot if negs == pos.len() => true,
                Predicate::IsNot => {
                    return Err("negation phrasing not supported by the clause".into())
                }
                _ => return Err("unsupported predicate for field".into()),
            };
            if want_size {
                Stmt::Size { negated, value: v }
            } else {
                Stmt::Firmness { negated, value: v }
            }
        }
        TermField::ReturnPeriod => match raw.predicate {
            Predicate::None => {
                if !has_any_phrase(&toks, RETURN_NONE_PHRASES) {
                    return Err("no-returns phrasing not supported by the clause".into());
                }
                Stmt::ReturnNone
            }
            Predicate::Is => {
                let q = raw.quantity_text.as_deref().unwrap_or("").trim();
                if q.is_empty() || q.len() > 4 || !q.bytes().all(|b| b.is_ascii_digit()) {
                    return Err("return quantity not a whole number".into());
                }
                if !duration_supported(q, raw.duration_unit, &toks) {
                    return Err("return quantity/unit not in the clause".into());
                }
                if toks.iter().any(|t| is_negation_token(t)) {
                    return Err("the clause negates the return period".into());
                }
                let n: u32 = q.parse().map_err(|_| "bad quantity".to_string())?;
                let (lo, hi) = match raw.duration_unit {
                    DurUnit::Nights | DurUnit::Days => (n, n),
                    DurUnit::Weeks => (n * 7, n * 7),
                    DurUnit::Months => (n * 28, n * 31),
                    DurUnit::Years => (n * 365, n * 366),
                    DurUnit::NotApplicable => return Err("missing unit".into()),
                };
                Stmt::Return {
                    min_nights: lo,
                    max_nights: hi,
                }
            }
            _ => return Err("unsupported predicate for return period".into()),
        },
    })
}

fn validate_amount(raw: &RawTerm, evidence: &str) -> Result<Stmt, String> {
    let toks_v = tokens(evidence);
    let toks = &toks_v;
    if raw.predicate != Predicate::NotApplicable && raw.predicate != Predicate::Is {
        return Err("unsupported predicate for amount".into());
    }
    if has_any_phrase(toks, UNSUPPORTED_PRICE_PHRASES) {
        return Ok(Stmt::PriceUnresolved {
            reason: "unsupported price relationship (discount/percent/financing/tax)".into(),
        });
    }
    let charge_of = |k: AmountKindRaw| -> Result<Option<Charge>, String> {
        Ok(match k {
            AmountKindRaw::Item => Some(Charge::Item),
            AmountKindRaw::Delivery => {
                if !has_any_phrase(toks, &["delivery", "shipping"]) {
                    return Err("delivery not mentioned in quote".into());
                }
                Some(Charge::Delivery)
            }
            AmountKindRaw::Extra => {
                if raw.extra_label == ExtraLabel::Other
                    || raw.extra_label == ExtraLabel::NotApplicable
                {
                    return Ok(Some(Charge::Extra(ExtraLabel::Other)));
                }
                if !has_any_phrase(toks, extra_words(raw.extra_label)) {
                    return Err("charge label not in quote".into());
                }
                Some(Charge::Extra(raw.extra_label))
            }
            _ => None,
        })
    };
    let Some(text) = raw.amount_text.as_deref() else {
        // mentioned without an amount
        return match charge_of(raw.amount_kind)? {
            Some(Charge::Extra(ExtraLabel::Other)) => Ok(Stmt::PriceUnresolved {
                reason: "additional charge with unsupported label".into(),
            }),
            Some(c) => Ok(Stmt::ChargeMentioned { charge: c }),
            None => Err("amount missing".into()),
        };
    };
    let cents = parse_amount_text(text).ok_or("amount text not an exact amount")?;
    let found = find_amounts(evidence);
    let Some(hit) = found.iter().find(|f| f.cents == cents) else {
        return Err("amount not in the supporting clause".into());
    };
    match raw.currency {
        Currency::Usd if hit.usd_marked => {}
        Currency::Other => {
            return Ok(Stmt::PriceUnresolved {
                reason: "non-USD amount".into(),
            })
        }
        _ => {
            return Ok(Stmt::PriceUnresolved {
                reason: "currency not stated".into(),
            })
        }
    }
    if raw.relation == Relation::ComponentOfTotal
        && !has_any_phrase(
            toks,
            &["including", "includes", "of which", "that's", "that is"],
        )
    {
        // unsupported relationship claim: keep the amount only as lower-bound evidence
        return Ok(Stmt::Unspecified { cents });
    }
    Ok(match raw.amount_kind {
        AmountKindRaw::Total => {
            match amount_kind_supported(raw.amount_kind, raw.total_covers, toks) {
                Ok(Some(covers)) => Stmt::Total { cents, covers },
                _ => Stmt::Unspecified { cents }, // keep as lower-bound evidence
            }
        }
        AmountKindRaw::Unspecified | AmountKindRaw::NotApplicable => Stmt::Unspecified { cents },
        k => match charge_of(k) {
            Ok(Some(Charge::Extra(ExtraLabel::Other))) => Stmt::PriceUnresolved {
                reason: format!(
                    "additional charge {} with unsupported label",
                    fmt_cents(cents)
                ),
            },
            Ok(Some(c)) => Stmt::ChargeAmount {
                charge: c,
                cents,
                relation: if raw.relation == Relation::Separate
                    && has_any_phrase(toks, &["plus", "+", "on top", "additional", "extra"])
                {
                    Relation::Separate
                } else {
                    raw.relation
                },
            },
            _ => Stmt::Unspecified { cents },
        },
    })
}

const CORRECTION_MARKERS: &[&str] = &[
    "i mean",
    "sorry",
    "correction",
    "actually",
    "make that",
    "rather",
    "scratch that",
];

/// Apply in-turn corrections: a later value for the same slot replaces an earlier
/// one only when a correction marker appears between them in the transcript.
pub fn apply_in_turn_corrections(
    terms: Vec<ValidatedTerm>,
    transcript: &str,
) -> Vec<ValidatedTerm> {
    let norm = normalize(transcript);
    let pos_of = |t: &ValidatedTerm| -> Option<usize> {
        // position of the term's own value when it is an amount, else of the quote
        if let Stmt::ChargeAmount { cents, .. }
        | Stmt::Total { cents, .. }
        | Stmt::Unspecified { cents } = t.stmt
        {
            let amounts = find_amounts(transcript);
            if let Some(a) = amounts.iter().rev().find(|a| a.cents == cents) {
                return Some(normalize(&transcript[..a.start]).len());
            }
        }
        norm.find(&normalize(&t.quote))
    };
    let mut keep = vec![true; terms.len()];
    for j in 0..terms.len() {
        if !terms[j].correction {
            continue;
        }
        for i in 0..j {
            if !keep[i]
                || terms[i].stmt.slot() != terms[j].stmt.slot()
                || terms[i].stmt == terms[j].stmt
            {
                continue;
            }
            if let (Some(pi), Some(pj)) = (pos_of(&terms[i]), pos_of(&terms[j])) {
                if pi < pj {
                    let between = &norm[pi.min(norm.len())..pj.min(norm.len())];
                    if CORRECTION_MARKERS.iter().any(|m| between.contains(m)) {
                        keep[i] = false;
                    }
                }
            }
        }
    }
    terms
        .into_iter()
        .zip(keep)
        .filter_map(|(t, k)| k.then_some(t))
        .collect()
}

// ---------------------------------------------------------------------------
// Goals, facts, evaluation
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NegGoals {
    pub sizes: Vec<EnumValue>,
    pub firmness: Vec<EnumValue>,
    pub budget_cents: u64,
    pub min_return_nights: u32,
}

impl Default for NegGoals {
    fn default() -> Self {
        NegGoals {
            sizes: vec![EnumValue::Queen],
            firmness: vec![EnumValue::Firm],
            budget_cents: 120_000,
            min_return_nights: 100,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Eval {
    Satisfies(String),
    Violates(String),
    Unknown(String),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CostReport {
    pub lower_bound: u64,
    pub exact: Option<u64>,
    pub notes: Vec<String>,
}

/// Cost accounting (PLAN v4 §1) over the active offer's established statements.
pub fn evaluate_cost(stmts: &[&Stmt], budget: u64) -> (Eval, CostReport) {
    let mut values: BTreeMap<Charge, BTreeSet<u64>> = BTreeMap::new();
    let mut mentioned: BTreeSet<Charge> = BTreeSet::new();
    let mut totals: Vec<(u64, Covers)> = Vec::new();
    let mut unspecified: Vec<u64> = Vec::new();
    let mut unresolved: Vec<String> = Vec::new();
    // charges stated as additional ("plus $75 setup") are never covered by a total
    let mut separate: BTreeSet<Charge> = BTreeSet::new();
    for s in stmts {
        match s {
            Stmt::ChargeAmount {
                charge,
                cents,
                relation,
            } => {
                values.entry(*charge).or_default().insert(*cents);
                mentioned.insert(*charge);
                if *relation == Relation::Separate {
                    separate.insert(*charge);
                }
            }
            Stmt::ChargeMentioned { charge } => {
                mentioned.insert(*charge);
            }
            Stmt::Total { cents, covers } => totals.push((*cents, *covers)),
            Stmt::Unspecified { cents } => unspecified.push(*cents),
            Stmt::PriceUnresolved { reason } => unresolved.push(reason.clone()),
            _ => {}
        }
    }
    let extras: Vec<Charge> = mentioned
        .iter()
        .copied()
        .filter(|c| matches!(c, Charge::Extra(_)))
        .collect();
    let mut universe: BTreeSet<Charge> = [Charge::Item, Charge::Delivery].into_iter().collect();
    universe.extend(extras.iter().copied());
    let covers_set = |c: Covers| -> BTreeSet<Charge> {
        let all: BTreeSet<Charge> = match c {
            Covers::ItemDelivery => [Charge::Item, Charge::Delivery].into_iter().collect(),
            Covers::ItemOnly => [Charge::Item].into_iter().collect(),
            Covers::AllIn => universe.clone(),
            Covers::NotApplicable => BTreeSet::new(),
        };
        all.difference(&separate).copied().collect()
    };
    let known = |c: &Charge| -> Option<u64> {
        values.get(c).and_then(|s| {
            if s.len() == 1 {
                s.iter().next().copied()
            } else {
                None
            }
        })
    };
    let maxv = |c: &Charge| -> u64 {
        values
            .get(c)
            .and_then(|s| s.iter().next_back().copied())
            .unwrap_or(0)
    };

    let mut notes = Vec::new();
    let mut consistent: Vec<(u64, Covers)> = Vec::new();
    let mut inconsistent = false;
    for &(t, cov) in &totals {
        let comps = covers_set(cov);
        let sum_known: u64 = comps.iter().filter_map(known).sum();
        let all_known = comps.iter().all(|c| known(c).is_some());
        if sum_known == t || (sum_known < t && !all_known) {
            consistent.push((t, cov));
        } else {
            inconsistent = true;
            notes.push(format!(
                "total {} conflicts with its stated components",
                fmt_cents(t)
            ));
        }
    }
    // lower bound
    let mut lb: u64 = 0;
    for s in values.values() {
        if let Some(m) = s.iter().next_back() {
            lb = lb.max(*m);
        }
    }
    for &(t, _) in &totals {
        lb = lb.max(t);
    }
    for &u in &unspecified {
        lb = lb.max(u);
    }
    let sum_distinct: u64 = values.keys().map(maxv).sum();
    lb = lb.max(sum_distinct);
    for &(t, cov) in &consistent {
        let comps = covers_set(cov);
        let outside: u64 = values.keys().filter(|c| !comps.contains(c)).map(maxv).sum();
        lb = lb.max(t + outside);
    }
    // exact
    let multi: Vec<Charge> = values
        .iter()
        .filter(|(_, s)| s.len() > 1)
        .map(|(c, _)| *c)
        .collect();
    for c in &multi {
        notes.push(format!("conflicting values for {}", c.label()));
    }
    let mut exact: Option<u64> = None;
    if unresolved.is_empty() && multi.is_empty() && !inconsistent {
        let rank = |c: Covers| match c {
            Covers::AllIn => 3,
            Covers::ItemDelivery => 2,
            Covers::ItemOnly => 1,
            Covers::NotApplicable => 0,
        };
        let chosen = consistent.iter().max_by_key(|(_, c)| rank(*c)).copied();
        let candidate = match chosen {
            Some((t, cov)) => {
                let comps = covers_set(cov);
                let mut sum = Some(t);
                for c in universe.iter().filter(|c| !comps.contains(c)) {
                    sum = match (sum, known(c)) {
                        (Some(a), Some(b)) => Some(a + b),
                        _ => None,
                    };
                }
                // other totals must agree with the chosen one
                let agree = consistent.iter().all(|&(t2, c2)| {
                    if rank(c2) == rank(cov) {
                        t2 == t
                    } else {
                        t2 <= t
                    }
                });
                if agree {
                    sum
                } else {
                    notes.push("stated totals disagree".into());
                    None
                }
            }
            None => universe.iter().map(known).sum::<Option<u64>>(),
        };
        if let Some(c) = candidate {
            if unspecified.iter().all(|&u| u <= c) {
                exact = Some(c);
            } else {
                notes.push("a stated price exceeds the computed total".into());
            }
        }
    }
    let report = CostReport {
        lower_bound: lb,
        exact,
        notes: notes.clone(),
    };
    let eval = if lb > budget {
        Violates(format!(
            "cost at least {} > budget {}",
            fmt_cents(lb),
            fmt_cents(budget)
        ))
    } else if let Some(c) = exact {
        if c <= budget {
            Satisfies(format!(
                "total {} incl. delivery <= {}",
                fmt_cents(c),
                fmt_cents(budget)
            ))
        } else {
            Violates(format!(
                "total {} > budget {}",
                fmt_cents(c),
                fmt_cents(budget)
            ))
        }
    } else {
        let mut why: Vec<String> = unresolved
            .iter()
            .map(|r| format!("unresolved: {r}"))
            .collect();
        why.extend(notes);
        for c in &universe {
            let covered = consistent
                .iter()
                .any(|&(_, cov)| covers_set(cov).contains(c));
            if !covered && known(c).is_none() && !multi.contains(c) {
                why.push(format!("{} unknown", c.label()));
            }
        }
        if why.is_empty() {
            why.push("total cost not established".into());
        }
        Unknown(why.join("; "))
    };
    (eval, report)
}

use Eval::{Satisfies, Unknown, Violates};

fn eval_enum(facts: &[(bool, EnumValue)], accepted: &[EnumValue], what: &str) -> Eval {
    if facts.is_empty() {
        return Unknown(format!("{what} not stated"));
    }
    let mut violations = Vec::new();
    let mut satisfied = BTreeSet::new();
    let mut unknown = Vec::new();
    for &(neg, v) in facts {
        if neg {
            if accepted.len() == 1 && accepted[0] == v {
                violations.push(format!("{what} is not {}", v.label()));
            } else {
                unknown.push(format!(
                    "{what} 'not {}' does not establish the value",
                    v.label()
                ));
            }
        } else if accepted.contains(&v) {
            satisfied.insert(v);
        } else {
            violations.push(format!("{what} {} not accepted", v.label()));
        }
    }
    if !violations.is_empty() {
        return Violates(violations.join("; "));
    }
    if satisfied.len() == 1 && unknown.is_empty() {
        return Satisfies(format!(
            "{what} {}",
            satisfied.iter().next().map(|v| v.label()).unwrap_or("")
        ));
    }
    if satisfied.len() > 1 {
        return Unknown(format!("conflicting {what} statements"));
    }
    Unknown(unknown.join("; "))
}

fn eval_return(stmts: &[&Stmt], min: u32) -> Eval {
    let ranges: Vec<(u32, u32)> = stmts
        .iter()
        .filter_map(|s| match s {
            Stmt::Return {
                min_nights,
                max_nights,
            } => Some((*min_nights, *max_nights)),
            Stmt::ReturnNone => Some((0, 0)),
            _ => None,
        })
        .collect();
    if ranges.is_empty() {
        return Unknown("return policy not stated".into());
    }
    if let Some((lo, hi)) = ranges.iter().find(|(_, hi)| *hi < min) {
        return Violates(if *hi == 0 {
            "no returns".into()
        } else if lo == hi {
            format!("{hi}-night return < {min}")
        } else {
            format!("{lo}-{hi}-night return < {min}")
        });
    }
    if ranges.len() > 1 && ranges.windows(2).any(|w| w[0] != w[1]) {
        return Unknown("conflicting return-period statements".into());
    }
    let (lo, hi) = ranges[0];
    if lo >= min {
        Satisfies(format!("{lo}-night return >= {min}"))
    } else {
        Unknown(format!("return period {lo}-{hi} nights straddles {min}"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequirementResults {
    pub cost: Eval,
    pub size: Eval,
    pub firmness: Eval,
    pub ret: Eval,
    pub cost_report: CostReport,
}

impl RequirementResults {
    pub fn all(&self) -> [(&'static str, &Eval); 4] {
        [
            ("price", &self.cost),
            ("size", &self.size),
            ("firmness", &self.firmness),
            ("return", &self.ret),
        ]
    }
}

pub fn evaluate_requirements(stmts: &[&Stmt], goals: &NegGoals) -> RequirementResults {
    let (cost, cost_report) = evaluate_cost(stmts, goals.budget_cents);
    let sizes: Vec<(bool, EnumValue)> = stmts
        .iter()
        .filter_map(|s| {
            if let Stmt::Size { negated, value } = s {
                Some((*negated, *value))
            } else {
                None
            }
        })
        .collect();
    let firm: Vec<(bool, EnumValue)> = stmts
        .iter()
        .filter_map(|s| {
            if let Stmt::Firmness { negated, value } = s {
                Some((*negated, *value))
            } else {
                None
            }
        })
        .collect();
    RequirementResults {
        cost,
        size: eval_enum(&sizes, &goals.sizes, "size"),
        firmness: eval_enum(&firm, &goals.firmness, "firmness"),
        ret: eval_return(stmts, goals.min_return_nights),
        cost_report,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn term(field: TermField, quote: &str) -> RawTerm {
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
    fn amount(kind: AmountKindRaw, text: &str, quote: &str) -> RawTerm {
        RawTerm {
            amount_kind: kind,
            amount_text: Some(text.into()),
            currency: Currency::Usd,
            ..term(TermField::Amount, quote)
        }
    }
    fn v(raw: &RawTerm, transcript: &str) -> Stmt {
        validate_term(raw, transcript).expect("valid").stmt
    }
    fn cost(stmts: &[Stmt]) -> (Eval, CostReport) {
        let r: Vec<&Stmt> = stmts.iter().collect();
        evaluate_cost(&r, 120_000)
    }

    #[test]
    fn cost_case_components_and_delivered_total() {
        let t = "The mattress is $1,000, delivery is $150, so $1,150 delivered.";
        let s = vec![
            v(
                &amount(AmountKindRaw::Item, "1,000", "The mattress is $1,000"),
                t,
            ),
            v(
                &amount(AmountKindRaw::Delivery, "150", "delivery is $150"),
                t,
            ),
            v(
                &RawTerm {
                    total_covers: Covers::ItemDelivery,
                    ..amount(AmountKindRaw::Total, "1,150", "$1,150 delivered")
                },
                t,
            ),
        ];
        let (e, r) = cost(&s);
        assert_eq!(r.exact, Some(115_000), "{r:?}");
        assert!(matches!(e, Eval::Satisfies(_)), "{e:?}");
    }

    #[test]
    fn cost_case_total_including_stated_delivery() {
        let t = "It's $1,150 including a $100 delivery fee.";
        let s = vec![
            v(
                &RawTerm {
                    total_covers: Covers::ItemDelivery,
                    ..amount(AmountKindRaw::Total, "1,150", t)
                },
                t,
            ),
            v(
                &RawTerm {
                    relation: Relation::ComponentOfTotal,
                    ..amount(AmountKindRaw::Delivery, "100", t)
                },
                t,
            ),
        ];
        let (e, r) = cost(&s);
        assert_eq!(r.exact, Some(115_000), "{r:?}");
        assert!(matches!(e, Eval::Satisfies(_)));
    }

    #[test]
    fn cost_case_delivered_plus_setup_pauses() {
        let t = "$1,150 delivered plus $75 setup.";
        let s = vec![
            v(
                &RawTerm {
                    total_covers: Covers::ItemDelivery,
                    ..amount(AmountKindRaw::Total, "1,150", "$1,150 delivered")
                },
                t,
            ),
            v(
                &RawTerm {
                    extra_label: ExtraLabel::Setup,
                    relation: Relation::Separate,
                    ..amount(AmountKindRaw::Extra, "75", "plus $75 setup")
                },
                t,
            ),
        ];
        let (e, r) = cost(&s);
        assert_eq!(r.exact, Some(122_500));
        assert!(matches!(e, Eval::Violates(_)));
    }

    #[test]
    fn cost_case_bare_1399_pauses() {
        let t = "$1,399.";
        let s = vec![v(&amount(AmountKindRaw::Unspecified, "1,399", "$1,399"), t)];
        let (e, r) = cost(&s);
        assert_eq!(r.lower_bound, 139_900);
        assert!(matches!(e, Eval::Violates(_)));
    }

    #[test]
    fn cost_extra_cases() {
        // delivery stated twice without correction: two candidates
        let t = "Delivery is $100. Delivery is $150.";
        let s = vec![
            v(
                &amount(AmountKindRaw::Delivery, "100", "Delivery is $100"),
                t,
            ),
            v(
                &amount(AmountKindRaw::Delivery, "150", "Delivery is $150"),
                t,
            ),
        ];
        assert!(matches!(cost(&s).0, Eval::Unknown(_)));
        // inconsistent total
        let t = "$1,150 delivered. The mattress alone is $1,200.";
        let s = vec![
            v(
                &RawTerm {
                    total_covers: Covers::ItemDelivery,
                    ..amount(AmountKindRaw::Total, "1,150", "$1,150 delivered")
                },
                t,
            ),
            v(
                &amount(AmountKindRaw::Item, "1,200", "The mattress alone is $1,200"),
                t,
            ),
        ];
        let (e, r) = cost(&s);
        assert!(matches!(e, Eval::Unknown(_)), "{e:?} {r:?}");
        // discount unresolved
        let t = "$1,150 minus a $50 discount.";
        let s = [v(&amount(AmountKindRaw::Unspecified, "1,150", t), t)];
        assert!(matches!(&s[0], Stmt::PriceUnresolved { .. }));
        // plus delivery: delivery unknown
        let t = "$1,150 plus delivery.";
        let s = vec![
            v(
                &RawTerm {
                    total_covers: Covers::ItemOnly,
                    ..amount(AmountKindRaw::Total, "1,150", t)
                },
                t,
            ),
            v(
                &RawTerm {
                    amount_text: None,
                    ..amount(AmountKindRaw::Delivery, "0", t)
                },
                t,
            ),
        ];
        let (e, _) = cost(&s);
        assert!(
            matches!(e, Eval::Unknown(ref m) if m.contains("delivery")),
            "{e:?}"
        );
        // $1,150 plus $100 delivery -> $1,250 -> violates
        let t = "$1,150 plus $100 delivery.";
        let s = vec![
            v(&amount(AmountKindRaw::Item, "1,150", t), t),
            v(
                &RawTerm {
                    relation: Relation::Separate,
                    ..amount(AmountKindRaw::Delivery, "100", t)
                },
                t,
            ),
        ];
        assert!(matches!(cost(&s).0, Eval::Violates(_)));
    }

    #[test]
    fn not_firm_violates_and_supported_negation() {
        let t = "It's not firm.";
        let raw = RawTerm {
            predicate: Predicate::IsNot,
            enum_value: EnumValue::Firm,
            ..term(TermField::Firmness, "not firm")
        };
        let s = v(&raw, t);
        let r = evaluate_requirements(&[&s], &NegGoals::default());
        assert!(matches!(r.firmness, Eval::Violates(_)));
        // claiming "is firm" from a negated quote is rejected
        let raw = RawTerm {
            predicate: Predicate::Is,
            enum_value: EnumValue::Firm,
            ..term(TermField::Firmness, "not firm")
        };
        assert!(validate_term(&raw, t).is_err());
        // "extra firm" is not "firm"
        let raw = RawTerm {
            predicate: Predicate::Is,
            enum_value: EnumValue::Firm,
            ..term(TermField::Firmness, "extra firm")
        };
        assert!(validate_term(&raw, "an extra firm one").is_err());
    }

    #[test]
    fn return_units() {
        let mk = |q: &str, u: DurUnit, quote: &str| RawTerm {
            predicate: Predicate::Is,
            quantity_text: Some(q.into()),
            duration_unit: u,
            ..term(TermField::ReturnPeriod, quote)
        };
        let g = NegGoals::default();
        let e = |raw: RawTerm| {
            let s = v(&raw, &raw.quote.clone());
            evaluate_requirements(&[&s], &g).ret
        };
        assert!(matches!(
            e(mk("3", DurUnit::Months, "3 month return")),
            Eval::Violates(_)
        ));
        assert!(matches!(
            e(mk("4", DurUnit::Months, "4 months to return")),
            Eval::Satisfies(_)
        ));
        assert!(matches!(
            e(mk("100", DurUnit::Days, "100 days")),
            Eval::Satisfies(_)
        ));
        assert!(matches!(
            e(mk("14", DurUnit::Weeks, "14 weeks")),
            Eval::Violates(_)
        ));
        assert!(matches!(
            e(mk("30", DurUnit::Days, "30-day return")),
            Eval::Violates(_)
        ));
        assert!(matches!(
            e(mk("100", DurUnit::Nights, "100-night return")),
            Eval::Satisfies(_)
        ));
    }

    #[test]
    fn in_turn_correction_vs_contradiction() {
        let t = "$1,150 delivered... I mean $1,250 delivered.";
        let a = RawTerm {
            total_covers: Covers::ItemDelivery,
            ..amount(AmountKindRaw::Total, "1,150", "$1,150 delivered")
        };
        let b = RawTerm {
            total_covers: Covers::ItemDelivery,
            corrects_earlier_value_in_turn: true,
            ..amount(AmountKindRaw::Total, "1,250", "I mean $1,250 delivered")
        };
        let ts = vec![validate_term(&a, t).unwrap(), validate_term(&b, t).unwrap()];
        let kept = apply_in_turn_corrections(ts, t);
        assert_eq!(kept.len(), 1);
        let s: Vec<&Stmt> = kept.iter().map(|k| &k.stmt).collect();
        assert!(matches!(evaluate_cost(&s, 120_000).0, Eval::Violates(_)));
        // without a marker both remain -> any violating candidate pauses
        let t2 = "$1,150 delivered or $1,250 delivered.";
        let a2 = RawTerm {
            quote: "$1,150 delivered".into(),
            ..a.clone()
        };
        let b2 = RawTerm {
            quote: "$1,250 delivered".into(),
            ..b.clone()
        };
        let ts = vec![
            validate_term(&a2, t2).unwrap(),
            validate_term(&b2, t2).unwrap(),
        ];
        let kept = apply_in_turn_corrections(ts, t2);
        assert_eq!(kept.len(), 2);
        let s: Vec<&Stmt> = kept.iter().map(|k| &k.stmt).collect();
        assert!(matches!(evaluate_cost(&s, 120_000).0, Eval::Violates(_)));
        // correction downward
        let t3 = "$1,250 delivered, sorry, $1,150 delivered.";
        let a3 = RawTerm {
            quote: "$1,250 delivered".into(),
            amount_text: Some("1,250".into()),
            ..a.clone()
        };
        let b3 = RawTerm {
            quote: "$1,150 delivered".into(),
            amount_text: Some("1,150".into()),
            ..b.clone()
        };
        let ts = vec![
            validate_term(&a3, t3).unwrap(),
            validate_term(&b3, t3).unwrap(),
        ];
        let kept = apply_in_turn_corrections(ts, t3);
        assert_eq!(kept.len(), 1);
        assert!(matches!(kept[0].stmt, Stmt::Total { cents: 115_000, .. }));
    }

    #[test]
    fn schema_is_valid_json_object() {
        let s = extraction_schema();
        assert_eq!(s["type"], "object");
    }
}
