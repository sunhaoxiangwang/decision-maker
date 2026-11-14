//! Regression tests converted from the code-review probes
//! (/private/tmp/vexo-review-probes-extra-20261114.rs) plus related fixes.

use copilot_core::contextual::{ChoiceAnswer, NoulVerdict};
use copilot_core::contract::*;
use copilot_core::distance::*;
use copilot_core::limits::*;
use copilot_core::lunch::{self, *};
use copilot_core::negotiation::*;
use copilot_core::session::*;
use std::collections::{BTreeMap, BTreeSet};

fn raw(field: TermField, quote: &str) -> RawTerm {
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
fn total(q: &str, n: &str) -> RawTerm {
    RawTerm {
        amount_kind: AmountKindRaw::Total,
        total_covers: Covers::ItemDelivery,
        amount_text: Some(n.into()),
        currency: Currency::Usd,
        predicate: Predicate::Is,
        ..raw(TermField::Amount, q)
    }
}
fn full(n: &str) -> (String, NegExtraction) {
    let q = format!("${n} delivered");
    let text = format!("{q}, queen, firm, 100-night return");
    (
        text,
        NegExtraction {
            turn_kind: TurnKind::NewOffer,
            amends_offer_id: None,
            terms: vec![
                total(&q, n),
                RawTerm {
                    predicate: Predicate::Is,
                    enum_value: EnumValue::Queen,
                    ..raw(TermField::Size, "queen")
                },
                RawTerm {
                    predicate: Predicate::Is,
                    enum_value: EnumValue::Firm,
                    ..raw(TermField::Firmness, "firm")
                },
                RawTerm {
                    predicate: Predicate::Is,
                    quantity_text: Some("100".into()),
                    duration_unit: DurUnit::Nights,
                    ..raw(TermField::ReturnPeriod, "100-night return")
                },
            ],
        },
    )
}
fn accept(s: &mut Session, text: &str, cid: &str, now: u64) -> Result<u64, ApiError> {
    s.accept_turn(AcceptReq {
        mode: s.mode,
        source_id: "ui".into(),
        client_turn_id: cid.into(),
        source_seq: None,
        text: text.into(),
        transcript_source: "typed".into(),
        speaker: None,
        input_source: "typed".into(),
        now_ms: now,
    })
    .map(|a| match a {
        Accepted::New(q) | Accepted::Duplicate(q) => q,
    })
}
fn eval(s: &mut Session, seq: Option<u64>, now: u64) {
    if let EvalStart::NeedsContextual(c) = s.start_evaluation(seq, None, now, u64::MAX) {
        s.commit_contextual(
            *c,
            ContextualResult::Noul(Ok(NoulVerdict::Clear { p_yes: 0.05 })),
            None,
            1,
            now,
        );
    }
}
fn apply_at(s: &mut Session, text: &str, e: &NegExtraction, cid: &str, now: u64) -> PhaseA {
    let seq = accept(s, text, cid, now).unwrap();
    s.queue.clear();
    let c = s.start_extraction(seq, now, u64::MAX).unwrap();
    let r = s.apply_negotiation(&c, e, "probe", now);
    if r == PhaseA::Applied {
        eval(s, Some(seq), now);
    }
    r
}
fn apply(s: &mut Session, text: &str, e: &NegExtraction, cid: &str) -> PhaseA {
    apply_at(s, text, e, cid, 0)
}
fn neg(id: &str, n: &str) -> Session {
    let mut s = Session::new(id, Mode::Negotiation, "i", "mock");
    s.set_goals(1, NegGoals::default()).unwrap();
    s.queue.clear();
    let (text, e) = full(n);
    assert_eq!(apply(&mut s, &text, &e, "t1"), PhaseA::Applied);
    s
}
fn items(s: &Session) -> Vec<(u64, u64)> {
    s.active
        .as_ref()
        .unwrap()
        .facts
        .iter()
        .filter(|f| !f.confirmed)
        .map(|f| (f.id, f.version))
        .collect()
}
fn confirm_at(s: &mut Session, now: u64) {
    let id = s.active.as_ref().unwrap().id.clone();
    s.confirm_facts(s.state_rev, &id, &items(s), now).unwrap();
    s.queue.clear();
    eval(s, None, now);
}
fn confirm(s: &mut Session) {
    confirm_at(s, 0)
}
fn mem() -> MemoryView {
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
                    distance: Distance::parse("1", DistUnit::Mi).unwrap(),
                },
                strength: Strength::Hard,
                note: String::new(),
                provenance: "test".into(),
            },
        ],
    }
}
fn lunch_session() -> Session {
    let mut s = Session::new("l", Mode::Lunch, "i", "mock");
    let l = s.lunch.as_mut().unwrap();
    l.load_fixture();
    l.setup_confirmed = true;
    s
}
fn out(r: LunchRules) -> (Outcome, Option<String>) {
    match r {
        LunchRules::Final(d) => (d.outcome, d.selected_option),
        LunchRules::Choice { .. } => (Outcome::Ask, None),
    }
}

#[test]
fn negation_in_original_clause_is_not_affirmative() {
    let f = RawTerm {
        predicate: Predicate::Is,
        enum_value: EnumValue::Firm,
        ..raw(TermField::Firmness, "firm")
    };
    assert!(validate_term(&f, "It is not firm.").is_err());
    // the negative reading is supported and violates the firm goal
    let n = RawTerm {
        predicate: Predicate::IsNot,
        enum_value: EnumValue::Firm,
        ..raw(TermField::Firmness, "firm")
    };
    let v = validate_term(&n, "It is not firm.").unwrap();
    let r = evaluate_requirements(&[&v.stmt], &NegGoals::default());
    assert!(matches!(r.firmness, Eval::Violates(_)));
    // a value word in two clauses with opposite readings is unresolved
    assert!(validate_term(&f, "This one is not firm, that one is firm.").is_err());
}

#[test]
fn cross_session_confirmation_is_rejected() {
    let a = neg("session-A", "1,150");
    let mut b = neg("session-B", "1,000");
    let id = a.active.as_ref().unwrap().id.clone();
    assert!(b.confirm_facts(a.state_rev, &id, &items(&a), 0).is_err());
    assert!(b
        .active
        .as_ref()
        .unwrap()
        .facts
        .iter()
        .all(|f| !f.confirmed));
    assert_ne!(a.active.as_ref().unwrap().id, b.active.as_ref().unwrap().id);
}

#[test]
fn rejected_critical_interpretation_blocks_until_corrected() {
    let mut s = neg("s", "1,150");
    confirm(&mut s);
    assert_eq!(s.snapshot("p").outcome, Outcome::Proceed);
    let id = s.active.as_ref().unwrap().id.clone();
    let bad = RawTerm {
        predicate: Predicate::Is,
        enum_value: EnumValue::Firm,
        ..raw(TermField::Firmness, "not firm")
    };
    let e = NegExtraction {
        turn_kind: TurnKind::Amendment,
        amends_offer_id: Some(id.clone()),
        terms: vec![total("$1,150 delivered", "1,150"), bad],
    };
    assert_eq!(
        apply(&mut s, "$1,150 delivered, not firm", &e, "t2"),
        PhaseA::Applied
    );
    confirm(&mut s);
    assert_eq!(s.latest_turn().unwrap().rejected_terms.len(), 1);
    assert_eq!(s.active.as_ref().unwrap().open_unresolved().len(), 1);
    assert_ne!(s.snapshot("p").outcome, Outcome::Proceed);
    // explicit correction resolves it
    let rev = s.state_rev;
    s.correct_value(rev, &id, Correction::Firmness(EnumValue::Firm), 0)
        .unwrap();
    s.queue.clear();
    eval(&mut s, None, 0);
    assert_eq!(s.snapshot("p").outcome, Outcome::Proceed);
}

#[test]
fn oversized_extraction_is_rejected_not_truncated() {
    let mut s = Session::new("many", Mode::Negotiation, "i", "mock");
    s.set_goals(1, NegGoals::default()).unwrap();
    s.queue.clear();
    let (mut text, mut e) = full("1,150");
    for _ in 0..28 {
        e.terms.push(RawTerm {
            predicate: Predicate::Is,
            enum_value: EnumValue::Queen,
            ..raw(TermField::Size, "queen")
        });
    }
    text.push_str("; also $1,399 delivered");
    e.terms.push(total("$1,399 delivered", "1,399"));
    assert_eq!(e.terms.len(), 33);
    let r = apply(&mut s, &text, &e, "many");
    assert!(
        matches!(r, PhaseA::Invalid(ref err) if err.code == "extraction_invalid_output"),
        "{r:?}"
    );
    assert!(s.active.is_none(), "nothing applied");
}

#[test]
fn all_in_plus_separate_charge_stays_additional() {
    let txt = "$1,150 all-in plus $75 setup";
    let mut t = total("$1,150 all-in", "1,150");
    t.total_covers = Covers::AllIn;
    let x = RawTerm {
        amount_kind: AmountKindRaw::Extra,
        extra_label: ExtraLabel::Setup,
        relation: Relation::Separate,
        amount_text: Some("75".into()),
        currency: Currency::Usd,
        predicate: Predicate::Is,
        ..raw(TermField::Amount, "plus $75 setup")
    };
    let v1 = validate_term(&t, txt).unwrap().stmt;
    let v2 = validate_term(&x, txt).unwrap().stmt;
    let (e, r) = evaluate_cost(&[&v1, &v2], 120_000);
    assert_eq!(r.lower_bound, 122_500);
    assert!(matches!(e, Eval::Violates(_)), "{e:?}");
    // an included component stays included
    let txt2 = "$1,150 all-in including $75 setup";
    let mut t2 = total("$1,150 all-in", "1,150");
    t2.total_covers = Covers::AllIn;
    let x2 = RawTerm {
        relation: Relation::ComponentOfTotal,
        quote: "including $75 setup".into(),
        ..x.clone()
    };
    let w1 = validate_term(&t2, txt2).unwrap().stmt;
    let w2 = validate_term(&x2, txt2).unwrap().stmt;
    let (e2, r2) = evaluate_cost(&[&w1, &w2], 120_000);
    assert_eq!(r2.exact, Some(115_000));
    assert!(matches!(e2, Eval::Satisfies(_)), "{e2:?}");
}

#[test]
fn lunch_contradiction_is_preserved_and_excludes() {
    let mut s = lunch_session();
    let l = s.lunch.as_mut().unwrap();
    l.apply_facts(
        vec![
            ValidLunchFact {
                option: 1,
                field: LunchField::Allergen {
                    allergen: Allergen::Peanut,
                    status: AllergenStatus::Contains,
                },
                excerpt: "Green Bowl contains peanuts".into(),
                truncated: false,
            },
            ValidLunchFact {
                option: 1,
                field: LunchField::Allergen {
                    allergen: Allergen::Peanut,
                    status: AllergenStatus::FreeOf,
                },
                excerpt: "Green Bowl is peanut-free".into(),
                truncated: false,
            },
        ],
        1,
        "probe",
    );
    for f in &mut l.options[1].facts {
        f.confirmed = true;
    }
    let (o, sel) = out(lunch::rules(
        l,
        Some(&mem()),
        &BTreeSet::new(),
        &BTreeSet::new(),
    ));
    assert_ne!(sel.as_deref(), Some("B"));
    assert_eq!(o, Outcome::NoEligibleOption);
}

#[test]
fn renaming_an_option_drops_previous_facts() {
    let mut s = lunch_session();
    let l = s.lunch.as_mut().unwrap();
    assert!(!l.options[1].facts.is_empty());
    l.rename_option(1, "New Place");
    assert!(l.options[1].facts.is_empty());
    let (o, _) = out(lunch::rules(
        l,
        Some(&mem()),
        &BTreeSet::new(),
        &BTreeSet::new(),
    ));
    assert_ne!(o, Outcome::Select);
}

#[test]
fn recovery_or_stale_memory_never_commits_a_selection() {
    let mut s = lunch_session();
    let m = mem();
    s.lunch.as_mut().unwrap().set_user_fact(
        2,
        LunchField::Distance {
            distance: Distance::parse("0.8", DistUnit::Mi).unwrap(),
        },
        "user",
    );
    let ctx = match s.start_evaluation(None, Some(&m), 0, u64::MAX) {
        EvalStart::NeedsContextual(c) => *c,
        other => panic!("choice expected, got {other:?}"),
    };
    s.memory_changed(1);
    let mut recovery = m.clone();
    recovery.recovery_required = Some("disk_state_unknown".into());
    let answer = ChoiceAnswer {
        choice: "B".into(),
        probabilities: BTreeMap::from([("B".into(), 0.8), ("C".into(), 0.2)]),
        confidence: 0.8,
    };
    let c = s.commit_contextual(
        ctx,
        ContextualResult::Choice(Ok(answer)),
        Some(&recovery),
        1,
        0,
    );
    assert_eq!(c, Commit::Stale);
    assert_ne!(s.snapshot("p").outcome, Outcome::Select);
}

#[test]
fn sole_eligible_path_rejects_a_stale_memory_view() {
    let mut s = lunch_session();
    let old = mem();
    s.memory_changed(2); // version 2 published (adds a sesame allergy)
    assert!(matches!(
        s.start_evaluation(None, Some(&old), 0, u64::MAX),
        EvalStart::Skipped
    ));
    assert_ne!(s.snapshot("p").outcome, Outcome::Select);
}

#[test]
fn offer_turn_list_is_bounded_and_context_limit_holds() {
    let mut s = neg("retention", "1,150");
    confirm(&mut s);
    let e = NegExtraction {
        turn_kind: TurnKind::Irrelevant,
        amends_offer_id: None,
        terms: vec![],
    };
    for i in 0..200 {
        s.queue.clear();
        apply(&mut s, "Thanks", &e, &format!("r-{i}"));
    }
    assert!(s.turns.len() <= TURN_RECORDS_PER_SESSION);
    assert!(s.active.as_ref().unwrap().turn_seqs.len() <= CONTEXT_MAX_TURNS + 1);
    assert_ne!(
        s.snapshot("p").outcome,
        Outcome::Proceed,
        "context limit blocks PROCEED"
    );
}

#[test]
fn barriers_are_bounded_and_remain_discardable() {
    let mut s = Session::new("failures", Mode::Negotiation, "i", "mock");
    s.set_goals(1, NegGoals::default()).unwrap();
    s.queue.clear();
    let mut refused = 0;
    for i in 0..80 {
        match accept(&mut s, "offer", &format!("f-{i}"), 0) {
            Ok(seq) => {
                s.queue.clear();
                s.start_extraction(seq, 0, u64::MAX);
                s.fail_before_apply(seq, ApiError::new("test_error", "test", false), 0);
            }
            Err(e) => {
                assert_eq!(e.code, "too_many_unresolved_turns");
                refused += 1;
            }
        }
    }
    assert!(s.barriers.len() <= MAX_UNRESOLVED_TURNS);
    assert!(refused > 0);
    let oldest = *s.barriers.iter().next().unwrap();
    assert!(s.turn(oldest).is_some(), "barrier turn retained");
    assert!(s.discard_turn(s.state_rev, oldest).is_ok());
}

#[test]
fn vegan_fact_needs_vegan_evidence_and_time_roles_are_checked() {
    let l = LunchState::default();
    let mut options = l.options;
    options[1].name = "Green Bowl".into();
    let q = "Green Bowl has vegetarian options";
    let mk = |diet, field, pred, time: Option<&str>, quote: &str| RawLunchFact {
        option: "B".into(),
        field,
        predicate: pred,
        allergen: Allergen::NotApplicable,
        diet,
        distance_text: None,
        distance_unit: LUnit::NotApplicable,
        time_text: time.map(String::from),
        quote: quote.into(),
    };
    let e = LunchExtraction {
        turn_kind: LunchTurnKind::OptionFacts,
        facts: vec![mk(
            Diet::Vegan,
            LField::DietOptions,
            LPredicate::Has,
            None,
            q,
        )],
    };
    assert!(lunch::validate_extraction(&e, q, &options).0.is_empty());
    let e = LunchExtraction {
        turn_kind: LunchTurnKind::OptionFacts,
        facts: vec![mk(
            Diet::Vegetarian,
            LField::DietOptions,
            LPredicate::Has,
            None,
            q,
        )],
    };
    assert_eq!(lunch::validate_extraction(&e, q, &options).0.len(), 1);
    // "open until 1 pm" is a closing time, not an opening time
    let q2 = "Green Bowl is open until 1 pm";
    let e = LunchExtraction {
        turn_kind: LunchTurnKind::OptionFacts,
        facts: vec![mk(
            Diet::NotApplicable,
            LField::OpensAt,
            LPredicate::Is,
            Some("1 pm"),
            q2,
        )],
    };
    let (ok, rej) = lunch::validate_extraction(&e, q2, &options);
    assert!(ok.is_empty() && rej.len() == 1);
    let e = LunchExtraction {
        turn_kind: LunchTurnKind::OptionFacts,
        facts: vec![mk(
            Diet::NotApplicable,
            LField::ClosesAt,
            LPredicate::Is,
            Some("1 pm"),
            q2,
        )],
    };
    assert_eq!(lunch::validate_extraction(&e, q2, &options).0.len(), 1);
}

#[test]
fn generation_latency_includes_confirmation_wait() {
    let mut s = Session::new("lat", Mode::Negotiation, "i", "mock");
    s.set_goals(1, NegGoals::default()).unwrap();
    s.queue.clear();
    let (text, e) = full("1,150");
    assert_eq!(apply_at(&mut s, &text, &e, "t1", 1_000), PhaseA::Applied);
    assert_eq!(s.phase, Phase::AwaitingConfirmation);
    // the user takes three seconds to confirm
    confirm_at(&mut s, 4_200);
    let snap = s.snapshot("p");
    assert_eq!(snap.outcome, Outcome::Proceed);
    assert!(
        snap.timings_ms.e2e.unwrap() >= 3_000,
        "{:?}",
        snap.timings_ms
    );
    assert!(
        snap.timings_ms.human_wait.unwrap() >= 3_000,
        "{:?}",
        snap.timings_ms
    );
    assert!(snap.timings_ms.processing.unwrap() < 1_000);
}

#[test]
fn rejected_lunch_fact_blocks_selection_until_dismissed() {
    let mut s = lunch_session();
    let l = s.lunch.as_mut().unwrap();
    let ok = l.add_unresolved(
        &LunchRejected {
            option: Some(1),
            field: "allergen".into(),
            reason: "phrasing not supported".into(),
            excerpt: "Green Bowl is careful with nuts".into(),
        },
        3,
    );
    assert!(ok);
    let (o, sel) = out(lunch::rules(
        l,
        Some(&mem()),
        &BTreeSet::new(),
        &BTreeSet::new(),
    ));
    assert_eq!((o, sel), (Outcome::Ask, None));
    let id = l.unresolved[0].id;
    assert!(l.dismiss_unresolved(id, 1));
    let (o, sel) = out(lunch::rules(
        l,
        Some(&mem()),
        &BTreeSet::new(),
        &BTreeSet::new(),
    ));
    assert_eq!((o, sel.as_deref()), (Outcome::Select, Some("B")));
}

#[test]
fn hard_avoid_preferences_are_rejected() {
    let p = Preference {
        id: 1,
        version: 1,
        kind: PrefKind::Diet {
            diet: Diet::Vegetarian,
            prefer: false,
        },
        strength: Strength::Hard,
        note: String::new(),
        provenance: "test".into(),
    };
    assert!(p.validate().is_err());
    let bad_distance = Preference {
        id: 1,
        version: 1,
        kind: PrefKind::MaxWalk {
            distance: Distance {
                text: "1.0".into(),
                unit: DistUnit::Mi,
                numer: 5,
                decimals: 0,
            },
        },
        strength: Strength::Hard,
        note: String::new(),
        provenance: "test".into(),
    };
    assert!(bad_distance.validate().is_err());
}

#[test]
fn ambiguous_turn_can_be_resolved_as_new_offer() {
    let mut s = Session::new("amb", Mode::Negotiation, "i", "mock");
    s.set_goals(1, NegGoals::default()).unwrap();
    s.queue.clear();
    let text = "Would you take $1,150 delivered?";
    let e = NegExtraction {
        turn_kind: TurnKind::NewOffer,
        amends_offer_id: None,
        terms: vec![total("$1,150 delivered", "1,150")],
    };
    assert_eq!(apply(&mut s, text, &e, "a1"), PhaseA::Applied);
    let seq = *s.ambiguous.iter().next().unwrap();
    let rev = s.state_rev;
    s.resolve_ambiguous(rev, seq, AmbiguousResolution::NewOffer)
        .unwrap();
    assert!(s.ambiguous.is_empty());
    let o = s.active.as_ref().unwrap();
    assert_eq!(o.facts.len(), 1);
    assert!(
        !o.facts[0].confirmed,
        "resolved terms still need confirmation"
    );
}

#[test]
fn deadline_passed_applies_nothing() {
    let mut s = Session::new("dl", Mode::Negotiation, "i", "mock");
    s.set_goals(1, NegGoals::default()).unwrap();
    s.queue.clear();
    let (text, e) = full("1,150");
    let seq = accept(&mut s, &text, "d1", 0).unwrap();
    let c = s.start_extraction(seq, 0, 8_000).unwrap();
    assert_eq!(s.apply_negotiation(&c, &e, "probe", 8_001), PhaseA::Expired);
    assert!(s.active.is_none());
    // queue expiry
    let seq2 = accept(&mut s, &text, "d2", 0).unwrap();
    assert!(s
        .start_extraction(seq2, QUEUE_WAIT_MAX_MS + 1, u64::MAX)
        .is_none());
    assert_eq!(
        s.turn(seq2).unwrap().error.as_ref().unwrap().code,
        "queue_timeout"
    );
}

// ---------------------------------------------------------------------------
// Second local review: condition / interpretation limits
// ---------------------------------------------------------------------------

fn irrelevant_with(s: &mut Session, cid: &str, v: NoulVerdict) {
    let e = NegExtraction {
        turn_kind: TurnKind::Irrelevant,
        amends_offer_id: None,
        terms: vec![],
    };
    let seq = accept(s, "ok", cid, 0).unwrap();
    s.queue.clear();
    let c = s.start_extraction(seq, 0, u64::MAX).unwrap();
    assert_eq!(s.apply_negotiation(&c, &e, "probe", 0), PhaseA::Applied);
    if let EvalStart::NeedsContextual(c) = s.start_evaluation(Some(seq), None, 0, u64::MAX) {
        s.commit_contextual(*c, ContextualResult::Noul(Ok(v)), None, 1, 0);
    }
}

#[test]
fn conditions_are_reclaimed_over_seventeen_cycles() {
    let mut s = neg("cond", "1,150");
    confirm(&mut s);
    assert_eq!(s.snapshot("p").outcome, Outcome::Proceed);
    for i in 0..20 {
        irrelevant_with(
            &mut s,
            &format!("c{i}"),
            NoulVerdict::Concern { p_yes: 0.9 },
        );
        assert_eq!(
            s.snapshot("p").outcome,
            Outcome::Ask,
            "cycle {i}: concern must block"
        );
        let o = s.active.as_ref().unwrap();
        let c = o.unresolved_conditions()[0].clone();
        let id = o.id.clone();
        s.resolve_condition(s.state_rev, &id, c.id, c.version)
            .unwrap();
        s.queue.clear();
        eval(&mut s, None, 0);
        assert_eq!(
            s.snapshot("p").outcome,
            Outcome::Proceed,
            "cycle {i}: resolved"
        );
        assert!(s.active.as_ref().unwrap().conditions.len() <= CONDITIONS_PER_OFFER);
    }
    // the 21st concern is recorded and still blocks a later clean evaluation
    irrelevant_with(&mut s, "last", NoulVerdict::Concern { p_yes: 0.9 });
    irrelevant_with(&mut s, "clean", NoulVerdict::Clear { p_yes: 0.01 });
    assert_eq!(s.snapshot("p").outcome, Outcome::Ask);
}

#[test]
fn rejected_interpretations_are_bounded_and_still_block() {
    let mut s = neg("unres", "1,150");
    confirm(&mut s);
    let id = s.active.as_ref().unwrap().id.clone();
    for i in 0..20 {
        let mut terms = vec![RawTerm {
            predicate: Predicate::Is,
            quantity_text: Some("100".into()),
            duration_unit: DurUnit::Nights,
            ..raw(TermField::ReturnPeriod, "100-night return")
        }];
        for _ in 0..11 {
            terms.push(RawTerm {
                predicate: Predicate::Is,
                enum_value: EnumValue::Firm,
                ..raw(TermField::Firmness, "not firm")
            });
        }
        let e = NegExtraction {
            turn_kind: TurnKind::Amendment,
            amends_offer_id: Some(id.clone()),
            terms,
        };
        assert_eq!(
            apply(&mut s, "100-night return, not firm", &e, &format!("u{i}")),
            PhaseA::Applied
        );
    }
    let o = s.active.as_ref().unwrap();
    assert!(
        o.unresolved.len() <= UNRESOLVED_PER_OFFER,
        "{}",
        o.unresolved.len()
    );
    let ov = o
        .unresolved
        .iter()
        .find(|u| u.field == "overflow")
        .expect("overflow record");
    assert!(
        ov.open && ov.merged > 100,
        "evidence counted, not dropped: {}",
        ov.merged
    );
    let (uid, uv) = (ov.id, ov.version);
    confirm(&mut s);
    assert_ne!(s.snapshot("p").outcome, Outcome::Proceed);
    // dismissing entries frees room; the overflow record still blocks until dismissed
    let rev = s.state_rev;
    s.dismiss_unresolved(rev, &id, uid, uv).unwrap();
    assert!(s.active.as_ref().unwrap().open_unresolved().len() < UNRESOLVED_PER_OFFER);
}

#[test]
fn lunch_rejections_are_bounded_with_blocking_overflow() {
    let mut s = lunch_session();
    let l = s.lunch.as_mut().unwrap();
    for i in 0..40 {
        assert!(l.add_unresolved(
            &LunchRejected {
                option: Some(1),
                field: "allergen".into(),
                reason: format!("r{i}"),
                excerpt: "x".into()
            },
            i
        ));
    }
    assert!(l.unresolved.len() <= LUNCH_UNRESOLVED_MAX);
    assert!(l
        .unresolved
        .iter()
        .any(|u| u.field == "overflow" && u.open && u.option.is_none()));
    let (o, sel) = out(lunch::rules(
        l,
        Some(&mem()),
        &BTreeSet::new(),
        &BTreeSet::new(),
    ));
    assert_eq!((o, sel), (Outcome::Ask, None));
}

#[test]
fn memory_publication_counter_never_goes_back() {
    let mut s = lunch_session();
    s.memory_changed(5);
    let mut m = mem();
    m.version = 1; // an older file restored, published as publication 6
    m.publication = 6;
    s.memory_changed(6);
    assert!(matches!(
        s.start_evaluation(None, Some(&m), 0, u64::MAX),
        EvalStart::Committed
    ));
    assert_eq!(s.snapshot("p").outcome, Outcome::Select);
    // a stale view does not re-enqueue itself forever
    s.queue.clear();
    m.publication = 5;
    assert!(matches!(
        s.start_evaluation(None, Some(&m), 0, u64::MAX),
        EvalStart::Skipped
    ));
    assert!(s.queue.is_empty());
}

// ---------------------------------------------------------------------------
// Offer vs amendment: explicit "same deal" wording never replaces the offer
// ---------------------------------------------------------------------------

fn amt(kind: AmountKindRaw, n: &str, q: &str) -> RawTerm {
    RawTerm {
        amount_kind: kind,
        amount_text: Some(n.into()),
        currency: Currency::Usd,
        predicate: Predicate::Is,
        ..raw(TermField::Amount, q)
    }
}

/// Offer with separate item + delivery charges, all confirmed, plus an open
/// contextual concern recorded on it.
fn offer_with_charges_and_condition() -> Session {
    let mut s = Session::new("amend", Mode::Negotiation, "i", "mock");
    s.set_goals(1, NegGoals::default()).unwrap();
    s.queue.clear();
    let text = "The mattress is $1,000, delivery is $100, queen, firm, 100-night return.";
    let e = NegExtraction {
        turn_kind: TurnKind::NewOffer,
        amends_offer_id: None,
        terms: vec![
            amt(AmountKindRaw::Item, "1,000", "The mattress is $1,000"),
            amt(AmountKindRaw::Delivery, "100", "delivery is $100"),
            RawTerm {
                predicate: Predicate::Is,
                enum_value: EnumValue::Queen,
                ..raw(TermField::Size, "queen")
            },
            RawTerm {
                predicate: Predicate::Is,
                enum_value: EnumValue::Firm,
                ..raw(TermField::Firmness, "firm")
            },
            RawTerm {
                predicate: Predicate::Is,
                quantity_text: Some("100".into()),
                duration_unit: DurUnit::Nights,
                ..raw(TermField::ReturnPeriod, "100-night return")
            },
        ],
    };
    assert_eq!(apply(&mut s, text, &e, "o1"), PhaseA::Applied);
    let id = s.active.as_ref().unwrap().id.clone();
    s.confirm_facts(s.state_rev, &id, &items(&s), 0).unwrap();
    s.queue.clear();
    let EvalStart::NeedsContextual(c) = s.start_evaluation(None, None, 0, u64::MAX) else {
        panic!("contextual expected")
    };
    s.commit_contextual(
        *c,
        ContextualResult::Noul(Ok(NoulVerdict::Concern { p_yes: 0.8 })),
        None,
        1,
        0,
    );
    assert_eq!(s.active.as_ref().unwrap().unresolved_conditions().len(), 1);
    s
}

fn has_delivery(s: &Session) -> bool {
    s.active.as_ref().unwrap().facts.iter().any(|f| {
        matches!(
            f.stmt,
            Stmt::ChargeAmount {
                charge: Charge::Delivery,
                cents: 10_000,
                ..
            }
        )
    })
}

const SAME_DEAL: &str = "Same deal, $1,000 for the mattress, but with a 120-night return.";

fn same_deal_terms() -> Vec<RawTerm> {
    vec![
        amt(AmountKindRaw::Item, "1,000", "$1,000 for the mattress"),
        RawTerm {
            predicate: Predicate::Is,
            quantity_text: Some("120".into()),
            duration_unit: DurUnit::Nights,
            ..raw(TermField::ReturnPeriod, "120-night return")
        },
    ]
}

#[test]
fn explicit_amendment_repeating_a_price_keeps_charges_and_conditions() {
    let mut s = offer_with_charges_and_condition();
    let id = s.active.as_ref().unwrap().id.clone();
    let e = NegExtraction {
        turn_kind: TurnKind::Amendment,
        amends_offer_id: Some(id.clone()),
        terms: same_deal_terms(),
    };
    assert_eq!(apply(&mut s, SAME_DEAL, &e, "a1"), PhaseA::Applied);
    let o = s.active.as_ref().unwrap();
    assert_eq!(o.id, id, "same offer");
    assert!(has_delivery(&s), "existing delivery charge survives");
    assert_eq!(
        o.unresolved_conditions().len(),
        1,
        "open condition survives"
    );
    assert!(o.facts.iter().any(|f| matches!(
        f.stmt,
        Stmt::Return {
            min_nights: 120,
            ..
        }
    )));
    assert_ne!(s.snapshot("p").outcome, Outcome::Proceed);
}

#[test]
fn keep_terms_wording_blocks_a_conflicting_new_offer_reading() {
    let mut s = offer_with_charges_and_condition();
    let id = s.active.as_ref().unwrap().id.clone();
    let e = NegExtraction {
        turn_kind: TurnKind::NewOffer,
        amends_offer_id: None,
        terms: same_deal_terms(),
    };
    assert_eq!(apply(&mut s, SAME_DEAL, &e, "a2"), PhaseA::Applied);
    let o = s.active.as_ref().unwrap();
    assert_eq!(o.id, id, "not replaced");
    assert!(has_delivery(&s));
    assert_eq!(o.unresolved_conditions().len(), 1);
    assert!(
        !s.ambiguous.is_empty(),
        "conflicting classification is unclear"
    );
    assert_eq!(s.snapshot("p").outcome, Outcome::Ask);
    // the user can still resolve it explicitly as an amendment
    let seq = *s.ambiguous.iter().next().unwrap();
    let rev = s.state_rev;
    s.resolve_ambiguous(rev, seq, AmbiguousResolution::Amendment)
        .unwrap();
    assert!(has_delivery(&s));
    assert_eq!(s.active.as_ref().unwrap().unresolved_conditions().len(), 1);
}

#[test]
fn standalone_replacement_offer_still_replaces() {
    let mut s = offer_with_charges_and_condition();
    let old = s.active.as_ref().unwrap().id.clone();
    let text = "New price: $1,180 delivered, queen, firm, 100-night return.";
    let e = NegExtraction {
        turn_kind: TurnKind::NewOffer,
        amends_offer_id: None,
        terms: vec![
            total("$1,180 delivered", "1,180"),
            RawTerm {
                predicate: Predicate::Is,
                enum_value: EnumValue::Queen,
                ..raw(TermField::Size, "queen")
            },
            RawTerm {
                predicate: Predicate::Is,
                enum_value: EnumValue::Firm,
                ..raw(TermField::Firmness, "firm")
            },
            RawTerm {
                predicate: Predicate::Is,
                quantity_text: Some("100".into()),
                duration_unit: DurUnit::Nights,
                ..raw(TermField::ReturnPeriod, "100-night return")
            },
        ],
    };
    assert_eq!(apply(&mut s, text, &e, "n1"), PhaseA::Applied);
    let o = s.active.as_ref().unwrap();
    assert_ne!(o.id, old, "replaced");
    assert!(!has_delivery(&s), "old charges do not carry over");
    assert!(o.unresolved_conditions().is_empty());
    assert!(s.ambiguous.is_empty());
    assert!(keeps_other_terms("Same deal, but 120 nights") && !keeps_other_terms(text));
}
