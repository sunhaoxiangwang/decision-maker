//! Lunch: three user-supplied options, exact facts, confirmed preferences, rules
//! and Gemini Decisions choice handling (PLAN v2 §4.6, v3 §8, v4 §8).

use crate::contextual::{choice_is_decisive, ChoiceAnswer};
use crate::contract::{ApiError, Outcome};
use crate::distance::{DistUnit, Distance};
use crate::limits::{LUNCH_FACTS_PER_OPTION, LUNCH_UNRESOLVED_MAX};
use crate::session::Decision;
use crate::text::{
    excerpt, has_any_phrase, has_phrase, is_negation_token, normalize, support_clauses, tokens,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const LETTERS: [&str; 3] = ["A", "B", "C"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Allergen {
    Peanut,
    TreeNut,
    Shellfish,
    Sesame,
    Gluten,
    Dairy,
    Egg,
    Soy,
    Fish,
    NotApplicable,
}

impl Allergen {
    pub fn words(self) -> &'static [&'static str] {
        match self {
            Allergen::Peanut => &["peanut", "peanuts"],
            Allergen::TreeNut => &["tree nut", "tree nuts"],
            Allergen::Shellfish => &["shellfish"],
            Allergen::Sesame => &["sesame"],
            Allergen::Gluten => &["gluten"],
            Allergen::Dairy => &["dairy"],
            Allergen::Egg => &["egg", "eggs"],
            Allergen::Soy => &["soy"],
            Allergen::Fish => &["fish"],
            Allergen::NotApplicable => &[],
        }
    }
    pub fn label(self) -> &'static str {
        self.words().first().copied().unwrap_or("n/a")
    }
    pub fn parse(s: &str) -> Option<Allergen> {
        Some(match s {
            "peanut" => Allergen::Peanut,
            "tree_nut" => Allergen::TreeNut,
            "shellfish" => Allergen::Shellfish,
            "sesame" => Allergen::Sesame,
            "gluten" => Allergen::Gluten,
            "dairy" => Allergen::Dairy,
            "egg" => Allergen::Egg,
            "soy" => Allergen::Soy,
            "fish" => Allergen::Fish,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Diet {
    Vegetarian,
    Vegan,
    NotApplicable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AllergenStatus {
    Contains,
    FreeOf,
    MayContain,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum LunchField {
    Allergen {
        allergen: Allergen,
        status: AllergenStatus,
    },
    Diet {
        diet: Diet,
        has: bool,
    },
    Distance {
        distance: Distance,
    },
    OpensAt {
        minute: u16,
    },
    ClosesAt {
        minute: u16,
    },
    ClosedAllDay,
    OpenAllDay,
    /// User-entered same-day intervals (up to 4).
    Hours {
        intervals: Vec<(u16, u16)>,
    },
}

impl LunchField {
    pub fn key(&self) -> String {
        match self {
            LunchField::Allergen { allergen, .. } => format!("allergen:{allergen:?}"),
            LunchField::Diet { diet, .. } => format!("diet:{diet:?}"),
            LunchField::Distance { .. } => "distance".into(),
            LunchField::OpensAt { .. }
            | LunchField::ClosesAt { .. }
            | LunchField::ClosedAllDay
            | LunchField::OpenAllDay
            | LunchField::Hours { .. } => match self {
                LunchField::OpensAt { .. } => "opens".into(),
                LunchField::ClosesAt { .. } => "closes".into(),
                _ => "hours".into(),
            },
        }
    }
    pub fn describe(&self) -> String {
        match self {
            LunchField::Allergen { allergen, status } => format!(
                "{} {}",
                allergen.label(),
                match status {
                    AllergenStatus::Contains => "contained",
                    AllergenStatus::FreeOf => "free",
                    AllergenStatus::MayContain => "may be present",
                }
            ),
            LunchField::Diet { diet, has } => format!(
                "{} options {}",
                format!("{diet:?}").to_lowercase(),
                if *has { "yes" } else { "no" }
            ),
            LunchField::Distance { distance } => distance.display(),
            LunchField::OpensAt { minute } => format!("opens {}", hhmm(*minute)),
            LunchField::ClosesAt { minute } => format!("closes {}", hhmm(*minute)),
            LunchField::ClosedAllDay => "closed all day".into(),
            LunchField::OpenAllDay => "open all day".into(),
            LunchField::Hours { intervals } => format!(
                "open {}",
                intervals
                    .iter()
                    .map(|(a, b)| format!("{}-{}", hhmm(*a), hhmm(*b)))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }
}

pub fn hhmm(m: u16) -> String {
    format!("{:02}:{:02}", m / 60, m % 60)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LunchFact {
    pub id: u64,
    pub version: u64,
    pub field: LunchField,
    pub confirmed: bool,
    pub source_seq: Option<u64>,
    pub provenance: String,
    pub excerpt: String,
    pub excerpt_truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LunchOption {
    pub letter: String,
    pub name: String,
    pub version: u64,
    pub facts: Vec<LunchFact>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LunchState {
    pub date: String,
    pub timezone: String,
    pub window: (u16, u16),
    pub options: Vec<LunchOption>,
    pub setup_confirmed: bool,
    pub setup_version: u64,
    pub next_fact_id: u64,
    /// Rejected interpretations of option facts; open ones block selection.
    pub unresolved: Vec<LunchUnresolved>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LunchUnresolved {
    pub id: u64,
    pub version: u64,
    pub option: Option<usize>,
    pub field: String,
    pub reason: String,
    pub excerpt: String,
    pub source_seq: u64,
    pub open: bool,
    pub merged: u32,
}

/// A rejected lunch fact (kept as an unresolved interpretation).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LunchRejected {
    pub option: Option<usize>,
    pub field: String,
    pub reason: String,
    pub excerpt: String,
}

impl Default for LunchState {
    fn default() -> Self {
        LunchState {
            date: "2026-11-14".into(),
            timezone: "America/Los_Angeles".into(),
            window: (12 * 60 + 30, 13 * 60 + 15),
            options: LETTERS
                .iter()
                .map(|l| LunchOption {
                    letter: l.to_string(),
                    name: String::new(),
                    version: 1,
                    facts: vec![],
                })
                .collect(),
            setup_confirmed: false,
            setup_version: 1,
            next_fact_id: 1,
            unresolved: vec![],
        }
    }
}

impl LunchState {
    pub fn option_versions(&self) -> [u64; 3] {
        [
            self.options[0].version,
            self.options[1].version,
            self.options[2].version,
        ]
    }
    pub fn prompt_context(&self) -> String {
        self.options
            .iter()
            .map(|o| format!("{} = {}", o.letter, o.name))
            .collect::<Vec<_>>()
            .join("; ")
    }
    /// Add transcript evidence. Nothing is overwritten: contradictory statements
    /// (e.g. "contains peanuts" and "peanut-free") are both kept and evaluated as a
    /// conflict. Only an explicit user entry replaces facts for a key.
    pub fn apply_facts(&mut self, facts: Vec<ValidLunchFact>, seq: u64, extractor: &str) {
        for v in facts {
            let o = &mut self.options[v.option];
            if o.facts.iter().any(|f| f.field == v.field && !f.confirmed) {
                continue; // identical unconfirmed evidence already present
            }
            let id = self.next_fact_id;
            self.next_fact_id += 1;
            let o = &mut self.options[v.option];
            o.facts.push(LunchFact {
                id,
                version: 1,
                field: v.field,
                confirmed: false,
                source_seq: Some(seq),
                provenance: format!("transcript turn {seq} ({extractor})"),
                excerpt: v.excerpt,
                excerpt_truncated: v.truncated,
            });
            o.version += 1;
        }
    }
    pub fn set_user_fact(&mut self, option: usize, field: LunchField, label: &str) {
        let id = self.next_fact_id;
        self.next_fact_id += 1;
        let o = &mut self.options[option];
        let key = field.key();
        o.facts.retain(|f| f.field.key() != key);
        let class = field_class(&field);
        for u in self
            .unresolved
            .iter_mut()
            .filter(|u| u.open && u.option == Some(option) && u.field == class)
        {
            u.open = false;
            u.version += 1;
        }
        o.facts.push(LunchFact {
            id,
            version: 1,
            field,
            confirmed: true,
            source_seq: None,
            provenance: label.into(),
            excerpt: String::new(),
            excerpt_truncated: false,
        });
        o.version += 1;
    }

    /// Room for these facts without silently dropping evidence.
    pub fn can_add(&self, facts: &[ValidLunchFact]) -> bool {
        (0..3).all(|i| {
            self.options[i].facts.len() + facts.iter().filter(|f| f.option == i).count()
                <= LUNCH_FACTS_PER_OPTION
        })
    }

    /// Record a rejected lunch fact. Dismissed records are reclaimed first; past
    /// the limit, readings merge into one session-wide blocking overflow record.
    pub fn add_unresolved(&mut self, r: &LunchRejected, seq: u64) -> bool {
        if self.unresolved.len() >= LUNCH_UNRESOLVED_MAX {
            self.unresolved.retain(|u| u.open);
        }
        if let Some(ov) = self
            .unresolved
            .iter_mut()
            .find(|u| u.open && u.field == "overflow")
        {
            ov.merged += 1;
            ov.version += 1;
            ov.reason = format!(
                "{} rejected readings beyond the {LUNCH_UNRESOLVED_MAX}-record limit (turns {}-{seq}); latest: {}",
                ov.merged, ov.source_seq, r.reason
            );
            ov.excerpt = r.excerpt.clone();
            return true;
        }
        if self.unresolved.len() + 1 >= LUNCH_UNRESOLVED_MAX {
            let id = self.next_fact_id;
            self.next_fact_id += 1;
            self.unresolved.push(LunchUnresolved {
                id,
                version: 1,
                option: None,
                field: "overflow".into(),
                reason: format!(
                    "rejected readings reached the {LUNCH_UNRESOLVED_MAX}-record limit; latest: {}",
                    r.reason
                ),
                excerpt: r.excerpt.clone(),
                source_seq: seq,
                open: true,
                merged: 1,
            });
            return true;
        }
        let id = self.next_fact_id;
        self.next_fact_id += 1;
        self.unresolved.push(LunchUnresolved {
            id,
            version: 1,
            option: r.option,
            field: r.field.clone(),
            reason: crate::text::bounded(&r.reason, 200).0,
            excerpt: r.excerpt.clone(),
            source_seq: seq,
            open: true,
            merged: 0,
        });
        true
    }

    /// Renaming an option creates a new option identity: its facts and open
    /// interpretations belong to the previous restaurant and are removed.
    pub fn rename_option(&mut self, i: usize, name: &str) {
        if self.options[i].name != name {
            self.options[i].name = name.to_string();
            self.options[i].facts.clear();
            self.options[i].version += 1;
            self.unresolved.retain(|u| u.option != Some(i));
        }
    }

    pub fn dismiss_unresolved(&mut self, id: u64, version: u64) -> bool {
        match self
            .unresolved
            .iter_mut()
            .find(|u| u.id == id && u.version == version && u.open)
        {
            Some(u) => {
                u.open = false;
                u.version += 1;
                true
            }
            None => false,
        }
    }

    /// Demo fixture (PLAN v2 §7 / v4 §8), entered as confirmed user facts.
    pub fn load_fixture(&mut self) {
        let names = ["Thai Basil", "Green Bowl", "Burger Barn"];
        for (i, n) in names.iter().enumerate() {
            self.options[i].name = n.to_string();
            self.options[i].facts.clear();
        }
        self.unresolved.clear();
        let p = "user entry (demo fixture)";
        let mi = |t: &str| Distance::parse(t, DistUnit::Mi).expect("fixture distance");
        self.set_user_fact(
            0,
            LunchField::Allergen {
                allergen: Allergen::Peanut,
                status: AllergenStatus::Contains,
            },
            p,
        );
        self.set_user_fact(
            0,
            LunchField::Distance {
                distance: mi("0.4"),
            },
            p,
        );
        self.set_user_fact(
            0,
            LunchField::Hours {
                intervals: vec![(660, 1260)],
            },
            p,
        );
        self.set_user_fact(
            0,
            LunchField::Diet {
                diet: Diet::Vegetarian,
                has: true,
            },
            p,
        );
        self.set_user_fact(
            1,
            LunchField::Allergen {
                allergen: Allergen::Peanut,
                status: AllergenStatus::FreeOf,
            },
            p,
        );
        self.set_user_fact(
            1,
            LunchField::Distance {
                distance: mi("1.0"),
            },
            p,
        );
        self.set_user_fact(
            1,
            LunchField::Hours {
                intervals: vec![(690, 840)],
            },
            p,
        );
        self.set_user_fact(
            1,
            LunchField::Diet {
                diet: Diet::Vegetarian,
                has: true,
            },
            p,
        );
        self.set_user_fact(
            2,
            LunchField::Allergen {
                allergen: Allergen::Peanut,
                status: AllergenStatus::FreeOf,
            },
            p,
        );
        self.set_user_fact(
            2,
            LunchField::Distance {
                distance: mi("1.8"),
            },
            p,
        );
        self.set_user_fact(
            2,
            LunchField::Hours {
                intervals: vec![(600, 1320)],
            },
            p,
        );
        self.set_user_fact(
            2,
            LunchField::Diet {
                diet: Diet::Vegetarian,
                has: false,
            },
            p,
        );
        self.window = (12 * 60 + 30, 13 * 60 + 15);
        self.setup_version += 1;
    }
}

// ---------------------------------------------------------------------------
// Preferences (confirmed memory view)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Strength {
    Hard,
    Soft,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum PrefKind {
    Allergen { allergen: Allergen },
    MaxWalk { distance: Distance },
    Diet { diet: Diet, prefer: bool },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Preference {
    pub id: u64,
    pub version: u64,
    pub kind: PrefKind,
    pub strength: Strength,
    pub note: String,
    /// Who confirmed it and how (e.g. "user entry via UI").
    pub provenance: String,
}

impl Preference {
    /// Semantic validation shared by UI entry and file load.
    pub fn validate(&self) -> Result<(), String> {
        if self.id == 0 || self.version == 0 {
            return Err("preference id/version must be positive".into());
        }
        if self.note.len() > crate::limits::PREF_TEXT_MAX {
            return Err("preference note too long".into());
        }
        if self.provenance.trim().is_empty() || self.provenance.len() > 128 {
            return Err("preference provenance missing or too long".into());
        }
        match (&self.kind, self.strength) {
            (
                PrefKind::Allergen {
                    allergen: Allergen::NotApplicable,
                },
                _,
            ) => Err("allergen missing".into()),
            (
                PrefKind::Diet {
                    diet: Diet::NotApplicable,
                    ..
                },
                _,
            ) => Err("diet missing".into()),
            (PrefKind::Diet { prefer: false, .. }, Strength::Hard) => {
                Err("hard \"avoid\" diet preferences are not supported".into())
            }
            (PrefKind::Allergen { .. }, Strength::Soft) => {
                Err("allergies must be hard constraints".into())
            }
            (PrefKind::MaxWalk { distance }, _) => {
                // the stored exact representation must re-derive from its text
                match Distance::parse(&distance.text, distance.unit) {
                    Some(d) if &d == distance => Ok(()),
                    _ => Err("stored distance is not a valid exact value".into()),
                }
            }
            _ => Ok(()),
        }
    }
}

impl Preference {
    pub fn describe(&self) -> String {
        let k = match &self.kind {
            PrefKind::Allergen { allergen } => format!("{} allergy", allergen.label()),
            PrefKind::MaxWalk { distance } => format!("walk at most {}", distance.display()),
            PrefKind::Diet { diet, prefer } => {
                format!(
                    "{} {} options",
                    if *prefer { "prefer" } else { "avoid" },
                    format!("{diet:?}").to_lowercase()
                )
            }
        };
        format!(
            "{k} ({})",
            if self.strength == Strength::Hard {
                "hard"
            } else {
                "soft"
            }
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MemoryView {
    /// File-level version (binds preference edit forms).
    pub version: u64,
    /// Monotonic publication counter: increases on every publication, including a
    /// restored older file or a profile reset, so sessions never see it go back.
    pub publication: u64,
    pub pending_change: bool,
    pub recovery_required: Option<String>,
    pub prefs: Vec<Preference>,
}

// ---------------------------------------------------------------------------
// Extraction contract
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LunchTurnKind {
    OptionFacts,
    PreferenceStatement,
    Irrelevant,
    Ambiguous,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LField {
    Allergen,
    DietOptions,
    Distance,
    OpensAt,
    ClosesAt,
    ClosedAllDay,
    OpenAllDay,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LPredicate {
    Contains,
    FreeOf,
    MayContain,
    Has,
    Lacks,
    Is,
    NotApplicable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LUnit {
    Mi,
    Ft,
    M,
    Km,
    NotApplicable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawLunchFact {
    pub option: String,
    pub field: LField,
    pub predicate: LPredicate,
    pub allergen: Allergen,
    pub diet: Diet,
    pub distance_text: Option<String>,
    pub distance_unit: LUnit,
    pub time_text: Option<String>,
    pub quote: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LunchExtraction {
    pub turn_kind: LunchTurnKind,
    pub facts: Vec<RawLunchFact>,
}

pub fn extraction_schema() -> serde_json::Value {
    serde_json::json!({"type":"object","additionalProperties":false,"required":["turn_kind","facts"],"properties":{
     "turn_kind":{"type":"string","enum":["option_facts","preference_statement","irrelevant","ambiguous"]},
     "facts":{"type":"array","items":{"type":"object","additionalProperties":false,
      "required":["option","field","predicate","allergen","diet","distance_text","distance_unit","time_text","quote"],
      "properties":{
       "option":{"type":"string","enum":["A","B","C"]},
       "field":{"type":"string","enum":["allergen","diet_options","distance","opens_at","closes_at","closed_all_day","open_all_day"]},
       "predicate":{"type":"string","enum":["contains","free_of","may_contain","has","lacks","is","not_applicable"]},
       "allergen":{"type":"string","enum":["peanut","tree_nut","shellfish","sesame","gluten","dairy","egg","soy","fish","not_applicable"]},
       "diet":{"type":"string","enum":["vegetarian","vegan","not_applicable"]},
       "distance_text":{"type":["string","null"]},
       "distance_unit":{"type":"string","enum":["mi","ft","m","km","not_applicable"]},
       "time_text":{"type":["string","null"]},
       "quote":{"type":"string"}}}}}})
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidLunchFact {
    pub option: usize,
    pub field: LunchField,
    pub excerpt: String,
    pub truncated: bool,
}

fn allergen_supported(
    toks: &[String],
    a: Allergen,
    p: LPredicate,
) -> Result<AllergenStatus, String> {
    if !has_any_phrase(toks, a.words()) {
        return Err("allergen not named in quote".into());
    }
    let words = a.words();
    let free = words.iter().any(|w| {
        has_any_phrase(
            toks,
            &[
                &format!("{w} free"),
                &format!("no {w}"),
                &format!("without {w}"),
                &format!("doesn't use {w}"),
                &format!("does not use {w}"),
                &format!("don't use {w}"),
                &format!("do not use {w}"),
                &format!("doesn't contain {w}"),
                &format!("does not contain {w}"),
                &format!("doesn't have {w}"),
                &format!("does not have {w}"),
                &format!("don't have {w}"),
                &format!("do not have {w}"),
            ],
        )
    });
    let contains = words.iter().any(|w| {
        has_any_phrase(
            toks,
            &[
                &format!("contains {w}"),
                &format!("has {w}"),
                &format!("uses {w}"),
                &format!("with {w}"),
                &format!("{w} sauce"),
                &format!("{w} oil"),
                &format!("{w} butter"),
            ],
        )
    });
    let may = has_any_phrase(
        toks,
        &[
            "may contain",
            "traces of",
            "shared kitchen",
            "shared fryer",
            "shared equipment",
            "cross contact",
        ],
    );
    if free && (contains || may) {
        return Err("quote supports both free-of and contains".into());
    }
    match p {
        LPredicate::FreeOf if free => Ok(AllergenStatus::FreeOf),
        LPredicate::Contains if contains && !free => Ok(AllergenStatus::Contains),
        LPredicate::MayContain if may => Ok(AllergenStatus::MayContain),
        _ => Err("allergen phrasing not supported".into()),
    }
}

/// "H[:MM] am/pm" or 24-hour "HH:MM". Bare hours without am/pm are unresolved.
pub fn parse_time(t: &str) -> Option<u16> {
    let s = t.trim().to_ascii_lowercase().replace('.', "");
    let (body, pm) = if let Some(b) = s.strip_suffix("pm") {
        (b.trim(), Some(true))
    } else if let Some(b) = s.strip_suffix("am") {
        (b.trim(), Some(false))
    } else {
        (s.as_str(), None)
    };
    let (h, m) = match body.split_once(':') {
        Some((h, m)) => (h.parse::<u16>().ok()?, {
            if m.len() != 2 {
                return None;
            }
            m.parse::<u16>().ok()?
        }),
        None => {
            pm?;
            (body.parse::<u16>().ok()?, 0)
        }
    };
    if m > 59 {
        return None;
    }
    let h24 = match pm {
        Some(true) if (1..=12).contains(&h) => (h % 12) + 12,
        Some(false) if (1..=12).contains(&h) => h % 12,
        None if body.contains(':') && h <= 23 => h,
        _ => return None,
    };
    Some(h24 * 60 + m)
}

fn option_mentioned(quote: &str, opt: &LunchOption) -> bool {
    let n = normalize(quote);
    (!opt.name.trim().is_empty() && n.contains(&normalize(&opt.name)))
        || has_phrase(
            &tokens(quote),
            &format!("option {}", opt.letter.to_lowercase()),
        )
}

fn diet_words(d: Diet) -> &'static [&'static str] {
    match d {
        Diet::Vegetarian => &["vegetarian", "veggie"],
        Diet::Vegan => &["vegan"],
        Diet::NotApplicable => &[],
    }
}

/// Unresolved-interpretation class of an extracted field.
pub fn lfield_class(f: LField) -> &'static str {
    match f {
        LField::Allergen => "allergen",
        LField::DietOptions => "diet",
        LField::Distance => "distance",
        LField::OpensAt | LField::ClosesAt | LField::ClosedAllDay | LField::OpenAllDay => "hours",
    }
}

pub fn field_class(f: &LunchField) -> &'static str {
    match f {
        LunchField::Allergen { .. } => "allergen",
        LunchField::Diet { .. } => "diet",
        LunchField::Distance { .. } => "distance",
        _ => "hours",
    }
}

/// Validate one claimed fact against one supporting clause of the transcript.
fn lunch_field_in(f: &RawLunchFact, evidence: &str) -> Result<LunchField, String> {
    let toks = tokens(evidence);
    let negated = toks.iter().any(|t| is_negation_token(t));
    Ok(match f.field {
        LField::Allergen => {
            if f.allergen == Allergen::NotApplicable {
                return Err("allergen missing".into());
            }
            let st = allergen_supported(&toks, f.allergen, f.predicate)?;
            LunchField::Allergen {
                allergen: f.allergen,
                status: st,
            }
        }
        LField::DietOptions => {
            let words = diet_words(f.diet);
            if words.is_empty() {
                return Err("diet missing".into());
            }
            let has_p: Vec<String> = words
                .iter()
                .flat_map(|w| {
                    [
                        format!("{w} options"),
                        format!("{w} menu"),
                        format!("{w} dishes"),
                    ]
                })
                .collect();
            let lacks_p: Vec<String> = words
                .iter()
                .flat_map(|w| {
                    [
                        format!("no {w}"),
                        format!("nothing {w}"),
                        format!("doesn't have {w}"),
                        format!("does not have {w}"),
                        format!("don't have {w}"),
                    ]
                })
                .collect();
            let has = has_p.iter().any(|p| has_phrase(&toks, p));
            let lacks = lacks_p.iter().any(|p| has_phrase(&toks, p));
            match f.predicate {
                LPredicate::Has if has && !lacks && !negated => LunchField::Diet {
                    diet: f.diet,
                    has: true,
                },
                LPredicate::Lacks if lacks && !has => LunchField::Diet {
                    diet: f.diet,
                    has: false,
                },
                _ => return Err("diet phrasing not supported for this diet".into()),
            }
        }
        LField::Distance => {
            let unit = match f.distance_unit {
                LUnit::Mi => DistUnit::Mi,
                LUnit::Ft => DistUnit::Ft,
                LUnit::M => DistUnit::M,
                LUnit::Km => DistUnit::Km,
                LUnit::NotApplicable => return Err("unit missing".into()),
            };
            let text = f.distance_text.as_deref().ok_or("distance missing")?;
            let d = Distance::parse(text, unit).ok_or("distance not in supported grammar")?;
            let present = toks.iter().enumerate().any(|(i, t)| {
                t == text.trim()
                    && toks
                        .get(i + 1)
                        .is_some_and(|u| unit.words().contains(&u.as_str()))
            });
            if !present {
                return Err("distance and unit not in the supporting clause".into());
            }
            if negated {
                return Err("the clause negates the distance".into());
            }
            LunchField::Distance { distance: d }
        }
        LField::OpensAt | LField::ClosesAt => {
            let tt = f.time_text.as_deref().ok_or("time missing")?;
            let m = parse_time(tt).ok_or("time format unsupported (needs am/pm or HH:MM)")?;
            if !normalize(evidence)
                .replace(' ', "")
                .contains(&normalize(tt).replace(' ', ""))
            {
                return Err("time not in the supporting clause".into());
            }
            if negated {
                return Err("the clause negates the time".into());
            }
            let close_role = toks
                .iter()
                .any(|t| matches!(t.as_str(), "close" | "closes" | "closing" | "until"));
            let open_role = toks
                .iter()
                .any(|t| matches!(t.as_str(), "opens" | "opening" | "from"))
                || has_phrase(&toks, "open at");
            if f.field == LField::OpensAt {
                if !open_role || close_role {
                    return Err("the clause does not state an opening time".into());
                }
                LunchField::OpensAt { minute: m }
            } else {
                if !close_role {
                    return Err("the clause does not state a closing time".into());
                }
                LunchField::ClosesAt { minute: m }
            }
        }
        LField::ClosedAllDay => {
            if negated || !has_any_phrase(&toks, &["closed today", "closed all day"]) {
                return Err("phrasing not supported".into());
            }
            LunchField::ClosedAllDay
        }
        LField::OpenAllDay => {
            if negated || !has_any_phrase(&toks, &["open 24 hours", "open all day"]) {
                return Err("phrasing not supported".into());
            }
            LunchField::OpenAllDay
        }
    })
}

/// Validate lunch facts against the FULL transcript, clause by clause.
pub fn validate_extraction(
    ext: &LunchExtraction,
    transcript: &str,
    options: &[LunchOption],
) -> (Vec<ValidLunchFact>, Vec<LunchRejected>) {
    let mut ok = Vec::new();
    let mut rej = Vec::new();
    if matches!(
        ext.turn_kind,
        LunchTurnKind::PreferenceStatement | LunchTurnKind::Irrelevant
    ) {
        return (ok, rej);
    }
    for f in &ext.facts {
        let (ex, tr) = excerpt(&f.quote);
        let idx = LETTERS.iter().position(|l| *l == f.option);
        let r = (|| -> Result<ValidLunchFact, String> {
            let idx = idx.ok_or("unknown option")?;
            let clauses = support_clauses(&f.quote, transcript);
            if clauses.is_empty() {
                return Err("quote not in transcript".into());
            }
            if !option_mentioned(&f.quote, &options[idx]) {
                return Err("quote does not name the option".into());
            }
            let mut field: Option<LunchField> = None;
            for c in &clauses {
                let v = lunch_field_in(f, c)
                    .map_err(|e| format!("{e} (context: \"{}\")", excerpt(c).0))?;
                match &field {
                    Some(prev) if prev != &v => {
                        return Err(
                            "the quoted words support different readings in different clauses"
                                .into(),
                        )
                    }
                    _ => field = Some(v),
                }
            }
            Ok(ValidLunchFact {
                option: idx,
                field: field.expect("one clause"),
                excerpt: ex.clone(),
                truncated: tr,
            })
        })();
        match r {
            Ok(v) => ok.push(v),
            Err(e) => rej.push(LunchRejected {
                option: idx,
                field: lfield_class(f.field).into(),
                reason: e,
                excerpt: ex.clone(),
            }),
        }
    }
    (ok, rej)
}

// ---------------------------------------------------------------------------
// Rules
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
enum Req {
    Sat(String),
    Vio(String),
    Unk(String),
}

pub enum LunchRules {
    Final(Decision),
    Choice {
        base: Decision,
        soft_prefs: String,
        options: BTreeMap<String, String>,
    },
}

fn facts_where(o: &LunchOption, pred: impl Fn(&LunchField) -> bool) -> Vec<&LunchFact> {
    o.facts.iter().filter(|f| pred(&f.field)).collect()
}

/// All-confirmed evidence satisfies; any unconfirmed piece leaves it unknown.
fn all_conf(fs: &[&LunchFact], ok: String) -> Req {
    if fs.iter().all(|f| f.confirmed) {
        Req::Sat(ok)
    } else {
        Req::Unk(format!("{ok} (unconfirmed)"))
    }
}

/// Evaluate every hard requirement over ALL evidence for each key: any violating
/// statement excludes the option even when other statements contradict it.
fn eval_option(o: &LunchOption, prefs: &[Preference], window: (u16, u16)) -> Vec<Req> {
    let mut out = Vec::new();
    for p in prefs.iter().filter(|p| p.strength == Strength::Hard) {
        match &p.kind {
            PrefKind::Allergen { allergen } => {
                let fs = facts_where(
                    o,
                    |f| matches!(f, LunchField::Allergen { allergen: a, .. } if a == allergen),
                );
                let bad = fs.iter().find(|f| {
                    matches!(
                        f.field,
                        LunchField::Allergen {
                            status: AllergenStatus::Contains | AllergenStatus::MayContain,
                            ..
                        }
                    )
                });
                out.push(match (bad, fs.is_empty()) {
                    (Some(f), _) => {
                        let conflict = fs.iter().any(|g| {
                            matches!(
                                g.field,
                                LunchField::Allergen {
                                    status: AllergenStatus::FreeOf,
                                    ..
                                }
                            )
                        });
                        Req::Vio(format!(
                            "{}{}",
                            match f.field {
                                LunchField::Allergen {
                                    status: AllergenStatus::Contains,
                                    ..
                                } => format!("contains {}", allergen.label()),
                                _ => format!("may contain {}", allergen.label()),
                            },
                            if conflict {
                                " (conflicting statements)"
                            } else {
                                ""
                            }
                        ))
                    }
                    (None, true) => Req::Unk(format!("{} status unknown", allergen.label())),
                    (None, false) => all_conf(&fs, format!("{}-free", allergen.label())),
                });
            }
            PrefKind::MaxWalk { distance: limit } => {
                let fs = facts_where(o, |f| matches!(f, LunchField::Distance { .. }));
                let ds: Vec<&Distance> = fs
                    .iter()
                    .filter_map(|f| {
                        if let LunchField::Distance { distance } = &f.field {
                            Some(distance)
                        } else {
                            None
                        }
                    })
                    .collect();
                let over: Vec<String> = ds
                    .iter()
                    .filter(|d| d.cmp_exact(limit) == std::cmp::Ordering::Greater)
                    .map(|d| d.display())
                    .collect();
                out.push(if !over.is_empty() {
                    Req::Vio(format!(
                        "{} > {} limit{}",
                        over.join(", "),
                        limit.display(),
                        if ds.len() > over.len() {
                            " (conflicting distances)"
                        } else {
                            ""
                        }
                    ))
                } else if ds.is_empty() {
                    Req::Unk("distance unknown".into())
                } else {
                    all_conf(
                        &fs,
                        format!(
                            "{} <= {}",
                            ds.iter()
                                .map(|d| d.display())
                                .collect::<Vec<_>>()
                                .join(", "),
                            limit.display()
                        ),
                    )
                });
            }
            PrefKind::Diet { diet, prefer: true } => {
                let fs = facts_where(
                    o,
                    |f| matches!(f, LunchField::Diet { diet: d, .. } if d == diet),
                );
                out.push(
                    if fs
                        .iter()
                        .any(|f| matches!(f.field, LunchField::Diet { has: false, .. }))
                    {
                        Req::Vio(format!("no {diet:?} options"))
                    } else if fs.is_empty() {
                        Req::Unk(format!("{diet:?} options unknown"))
                    } else {
                        all_conf(&fs, format!("{diet:?} options"))
                    },
                );
            }
            // hard "avoid" diet preferences are rejected at entry and on load
            PrefKind::Diet { prefer: false, .. } => {
                out.push(Req::Unk("unsupported hard avoid preference".into()))
            }
        }
    }
    // opening hours must cover the whole window; any violating evidence excludes
    let (ws, we) = window;
    let hours = facts_where(o, |f| {
        matches!(
            f,
            LunchField::Hours { .. }
                | LunchField::ClosedAllDay
                | LunchField::OpenAllDay
                | LunchField::OpensAt { .. }
                | LunchField::ClosesAt { .. }
        )
    });
    let mut vio = Vec::new();
    let (mut covered, mut open_at, mut close_at) = (false, None, None);
    for f in &hours {
        match &f.field {
            LunchField::ClosedAllDay => vio.push("closed on the lunch date".to_string()),
            LunchField::OpenAllDay => covered = true,
            LunchField::Hours { intervals } => {
                if intervals.iter().any(|(a, b)| *a <= ws && *b >= we) {
                    covered = true;
                } else {
                    vio.push(format!(
                        "hours {} do not cover {}-{}",
                        f.field.describe(),
                        hhmm(ws),
                        hhmm(we)
                    ));
                }
            }
            LunchField::OpensAt { minute } => {
                if *minute > ws {
                    vio.push(format!("opens {} after {}", hhmm(*minute), hhmm(ws)));
                }
                open_at = Some(*minute);
            }
            LunchField::ClosesAt { minute } => {
                if *minute < we {
                    vio.push(format!("closes {} before {}", hhmm(*minute), hhmm(we)));
                }
                close_at = Some(*minute);
            }
            _ => {}
        }
    }
    out.push(if !vio.is_empty() {
        Req::Vio(vio.join("; "))
    } else if covered || (open_at.is_some() && close_at.is_some()) {
        all_conf(&hours, format!("open over {}-{}", hhmm(ws), hhmm(we)))
    } else {
        Req::Unk("hours unknown".into())
    });
    out
}

pub fn rules(
    l: &LunchState,
    memory: Option<&MemoryView>,
    barriers: &BTreeSet<u64>,
    ambiguous: &BTreeSet<u64>,
) -> LunchRules {
    let Some(mem) = memory else {
        let mut d = Decision::simple(
            Outcome::Unavailable,
            "Preference memory unavailable.",
            "none",
        );
        d.error = Some(ApiError::new(
            "memory_unavailable",
            "preference memory unavailable",
            true,
        ));
        return LunchRules::Final(d);
    };
    if let Some(r) = &mem.recovery_required {
        let mut d = Decision::simple(
            Outcome::Unavailable,
            format!("Preferences need recovery before lunch decisions: {r}"),
            "none",
        );
        d.error = Some(ApiError::new("memory_recovery_required", r.clone(), false));
        return LunchRules::Final(d);
    }
    if mem.pending_change {
        return LunchRules::Final(Decision::simple(
            Outcome::None,
            "A preference change is being saved.",
            "none",
        ));
    }
    if l.options.len() != 3 || l.options.iter().any(|o| o.name.trim().is_empty()) {
        return LunchRules::Final(Decision::simple(
            Outcome::Ask,
            "Exactly three named options are required.",
            "rules",
        ));
    }
    if !barriers.is_empty() {
        let list = barriers
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        let mut d = Decision::simple(
            Outcome::Unavailable,
            format!("Turn(s) {list} were not processed; retry or discard."),
            "rules",
        );
        d.error = Some(ApiError::new(
            "unresolved_failed_turn",
            format!("unresolved failed turn(s): {list}"),
            true,
        ));
        return LunchRules::Final(d);
    }
    // conflicting confirmed preferences on the same attribute
    let mut diets: BTreeMap<String, BTreeSet<bool>> = BTreeMap::new();
    for p in &mem.prefs {
        if let PrefKind::Diet { diet, prefer } = &p.kind {
            diets
                .entry(format!("{diet:?}"))
                .or_default()
                .insert(*prefer);
        }
    }
    if diets.values().any(|s| s.len() > 1) {
        let mut d = Decision::simple(
            Outcome::Ask,
            "Ask: confirmed preferences conflict (prefer vs avoid); resolve them first.",
            "rules",
        );
        d.conflicts = vec!["conflicting diet preferences".into()];
        return LunchRules::Final(d);
    }
    let mut excluded = Vec::new();
    let mut undetermined = Vec::new();
    let mut eligible = Vec::new();
    if let Some(u) = l.unresolved.iter().find(|u| u.open && u.option.is_none()) {
        let mut d = Decision::simple(
            Outcome::Ask,
            format!(
                "Ask: unresolved statement: {} (\"{}\").",
                u.reason, u.excerpt
            ),
            "rules",
        );
        d.missing = vec![format!("unresolved: {}", u.reason)];
        return LunchRules::Final(d);
    }
    let mut explain = Vec::new();
    for (i, o) in l.options.iter().enumerate() {
        let mut reqs = eval_option(o, &mem.prefs, l.window);
        for u in l
            .unresolved
            .iter()
            .filter(|u| u.open && u.option == Some(i))
        {
            reqs.push(Req::Unk(format!(
                "unresolved {} interpretation: {}",
                u.field, u.reason
            )));
        }
        let vio: Vec<String> = reqs
            .iter()
            .filter_map(|r| {
                if let Req::Vio(m) = r {
                    Some(m.clone())
                } else {
                    None
                }
            })
            .collect();
        let unk: Vec<String> = reqs
            .iter()
            .filter_map(|r| {
                if let Req::Unk(m) = r {
                    Some(m.clone())
                } else {
                    None
                }
            })
            .collect();
        let sat: Vec<String> = reqs
            .iter()
            .filter_map(|r| {
                if let Req::Sat(m) = r {
                    Some(m.clone())
                } else {
                    None
                }
            })
            .collect();
        if !vio.is_empty() {
            explain.push(format!(
                "{} {} excluded: {}",
                o.letter,
                o.name,
                vio.join(", ")
            ));
            excluded.push(o.letter.clone());
        } else if !unk.is_empty() {
            explain.push(format!(
                "{} {} undetermined: {}",
                o.letter,
                o.name,
                unk.join(", ")
            ));
            undetermined.push(format!("{}: {}", o.letter, unk.join(", ")));
        } else {
            explain.push(format!(
                "{} {} eligible: {}",
                o.letter,
                o.name,
                sat.join(", ")
            ));
            eligible.push(o.letter.clone());
        }
    }
    let reason = explain.join(". ");
    if excluded.len() == 3 {
        let mut d = Decision::simple(
            Outcome::NoEligibleOption,
            format!("{reason}. No option qualifies."),
            "rules",
        );
        d.conflicts = excluded;
        return LunchRules::Final(d);
    }
    if !undetermined.is_empty() || !ambiguous.is_empty() {
        let mut d = Decision::simple(Outcome::Ask, format!("Ask: {reason}."), "rules");
        d.missing = undetermined;
        if !ambiguous.is_empty() {
            d.missing
                .push("unclear statement in the conversation".into());
        }
        d.awaiting_confirmation = d.missing.iter().all(|m| m.contains("unconfirmed"));
        return LunchRules::Final(d);
    }
    if eligible.len() == 1 {
        let mut d = Decision::simple(
            Outcome::Select,
            format!("{reason}. Only eligible option; selected by rules."),
            "rules_sole_eligible",
        );
        d.selected_option = Some(eligible[0].clone());
        return LunchRules::Final(d);
    }
    let soft: Vec<String> = mem
        .prefs
        .iter()
        .filter(|p| p.strength == Strength::Soft)
        .map(|p| p.describe())
        .collect();
    let soft_prefs = if soft.is_empty() {
        "none stated".to_string()
    } else {
        soft.join("; ")
    };
    let mut options = BTreeMap::new();
    for o in l.options.iter().filter(|o| eligible.contains(&o.letter)) {
        let facts: Vec<String> = o.facts.iter().map(|f| f.field.describe()).collect();
        options.insert(
            o.letter.clone(),
            format!("{}: {}", o.name, facts.join("; ")),
        );
    }
    let mut base = Decision::simple(Outcome::Ask, reason, "rules");
    base.missing = vec![];
    LunchRules::Choice {
        base,
        soft_prefs,
        options,
    }
}

pub fn decide_with_choice(mut d: Decision, a: &ChoiceAnswer, _l: Option<&LunchState>) -> Decision {
    d.decision_source = "gemini_decisions".into();
    let probs = a
        .probabilities
        .iter()
        .map(|(k, p)| format!("{k} {p:.2}"))
        .collect::<Vec<_>>()
        .join(", ");
    if choice_is_decisive(a) {
        d.outcome = Outcome::Select;
        d.selected_option = Some(a.choice.clone());
        d.reason = format!(
            "{}. Gemini Decisions returned {} (probabilities {}). The model returns probabilities, not reasons.",
            d.reason, a.choice, probs
        );
    } else {
        d.outcome = Outcome::Ask;
        d.selected_option = None;
        d.reason = format!(
            "{}. No clear preference among eligible options (probabilities {}).",
            d.reason, probs
        );
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem() -> MemoryView {
        let mi = |t: &str| Distance::parse(t, DistUnit::Mi).unwrap();
        MemoryView {
            version: 1,
            publication: 1,
            pending_change: false,
            recovery_required: None,
            prefs: vec![
                Preference {
                    id: 1,
                    version: 1,
                    kind: PrefKind::Allergen {
                        allergen: Allergen::Peanut,
                    },
                    strength: Strength::Hard,
                    note: String::new(),
                    provenance: "test".into(),
                },
                Preference {
                    id: 2,
                    version: 1,
                    kind: PrefKind::MaxWalk {
                        distance: mi("1.0"),
                    },
                    strength: Strength::Hard,
                    note: String::new(),
                    provenance: "test".into(),
                },
                Preference {
                    id: 3,
                    version: 1,
                    kind: PrefKind::Diet {
                        diet: Diet::Vegetarian,
                        prefer: true,
                    },
                    strength: Strength::Soft,
                    note: String::new(),
                    provenance: "test".into(),
                },
            ],
        }
    }

    fn fixture() -> LunchState {
        let mut l = LunchState::default();
        l.load_fixture();
        l.setup_confirmed = true;
        l
    }

    fn final_of(r: LunchRules) -> Decision {
        match r {
            LunchRules::Final(d) => d,
            LunchRules::Choice { .. } => panic!("expected final"),
        }
    }

    #[test]
    fn l1_sole_eligible_b() {
        let d = final_of(rules(
            &fixture(),
            Some(&mem()),
            &BTreeSet::new(),
            &BTreeSet::new(),
        ));
        assert_eq!(d.outcome, Outcome::Select);
        assert_eq!(d.selected_option.as_deref(), Some("B"));
        assert_eq!(d.decision_source, "rules_sole_eligible");
    }

    #[test]
    fn l2_boundary_101_no_eligible() {
        let mut l = fixture();
        l.set_user_fact(
            1,
            LunchField::Distance {
                distance: Distance::parse("1.01", DistUnit::Mi).unwrap(),
            },
            "user",
        );
        let d = final_of(rules(&l, Some(&mem()), &BTreeSet::new(), &BTreeSet::new()));
        assert_eq!(d.outcome, Outcome::NoEligibleOption);
        assert!(d.selected_option.is_none());
    }

    #[test]
    fn l3_two_eligible_needs_choice_and_letters_stable() {
        let mut l = fixture();
        l.set_user_fact(
            2,
            LunchField::Distance {
                distance: Distance::parse("0.8", DistUnit::Mi).unwrap(),
            },
            "user",
        );
        match rules(&l, Some(&mem()), &BTreeSet::new(), &BTreeSet::new()) {
            LunchRules::Choice { options, .. } => {
                assert_eq!(
                    options.keys().cloned().collect::<Vec<_>>(),
                    vec!["B".to_string(), "C".to_string()]
                )
            }
            _ => panic!("expected choice"),
        }
    }

    #[test]
    fn l4_missing_peanut_status_asks() {
        let mut l = fixture();
        l.options[1]
            .facts
            .retain(|f| !matches!(f.field, LunchField::Allergen { .. }));
        let d = final_of(rules(&l, Some(&mem()), &BTreeSet::new(), &BTreeSet::new()));
        assert_eq!(d.outcome, Outcome::Ask);
        assert!(d.selected_option.is_none());
    }

    #[test]
    fn l5_doesnt_use_peanuts_needs_confirmation() {
        let mut l = fixture();
        l.options[1]
            .facts
            .retain(|f| !matches!(f.field, LunchField::Allergen { .. }));
        let t = "Green Bowl doesn't use peanuts at all.";
        let ext = LunchExtraction {
            turn_kind: LunchTurnKind::OptionFacts,
            facts: vec![RawLunchFact {
                option: "B".into(),
                field: LField::Allergen,
                predicate: LPredicate::FreeOf,
                allergen: Allergen::Peanut,
                diet: Diet::NotApplicable,
                distance_text: None,
                distance_unit: LUnit::NotApplicable,
                time_text: None,
                quote: "Green Bowl doesn't use peanuts".into(),
            }],
        };
        let (ok, rej) = validate_extraction(&ext, t, &l.options);
        assert_eq!(ok.len(), 1, "{rej:?}");
        l.apply_facts(ok, 5, "mock");
        let d = final_of(rules(&l, Some(&mem()), &BTreeSet::new(), &BTreeSet::new()));
        assert_eq!(d.outcome, Outcome::Ask);
        // "careful with nuts" and "nut-free" are unresolved for peanut
        let ext2 = LunchExtraction {
            facts: vec![RawLunchFact {
                quote: "Green Bowl is nut-free".into(),
                ..ext.facts[0].clone()
            }],
            ..ext.clone()
        };
        let (ok2, _) = validate_extraction(&ext2, "Green Bowl is nut-free", &l.options);
        assert!(ok2.is_empty());
    }

    #[test]
    fn hours_and_times() {
        assert_eq!(parse_time("1 pm"), Some(780));
        assert_eq!(parse_time("1"), None);
        assert_eq!(parse_time("13:15"), Some(795));
        assert_eq!(parse_time("12:30 pm"), Some(750));
        let mut l = fixture();
        l.set_user_fact(
            1,
            LunchField::Hours {
                intervals: vec![(690, 780)],
            },
            "user",
        );
        let d = final_of(rules(&l, Some(&mem()), &BTreeSet::new(), &BTreeSet::new()));
        assert_eq!(d.outcome, Outcome::NoEligibleOption);
    }

    #[test]
    fn recovery_blocks_and_conflicting_prefs_ask() {
        let mut m = mem();
        m.recovery_required = Some("corrupt".into());
        let d = final_of(rules(
            &fixture(),
            Some(&m),
            &BTreeSet::new(),
            &BTreeSet::new(),
        ));
        assert_eq!(d.outcome, Outcome::Unavailable);
        let mut m = mem();
        m.prefs.push(Preference {
            id: 4,
            version: 1,
            kind: PrefKind::Diet {
                diet: Diet::Vegetarian,
                prefer: false,
            },
            strength: Strength::Soft,
            note: String::new(),
            provenance: "test".into(),
        });
        let d = final_of(rules(
            &fixture(),
            Some(&m),
            &BTreeSet::new(),
            &BTreeSet::new(),
        ));
        assert_eq!(d.outcome, Outcome::Ask);
    }

    #[test]
    fn may_contain_excludes() {
        let mut l = fixture();
        l.set_user_fact(
            1,
            LunchField::Allergen {
                allergen: Allergen::Peanut,
                status: AllergenStatus::MayContain,
            },
            "user",
        );
        let d = final_of(rules(&l, Some(&mem()), &BTreeSet::new(), &BTreeSet::new()));
        assert_eq!(d.outcome, Outcome::NoEligibleOption);
    }
}
