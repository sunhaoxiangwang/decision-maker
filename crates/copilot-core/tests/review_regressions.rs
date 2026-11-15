//! Regression tests converted from the code-review probes plus related fixes
//! (lunch rules, memory views and turn admission).

use copilot_core::contextual::ChoiceAnswer;
use copilot_core::contract::*;
use copilot_core::distance::*;
use copilot_core::limits::*;
use copilot_core::lunch::{self, *};
use copilot_core::session::*;
use std::collections::{BTreeMap, BTreeSet};

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
fn barriers_are_bounded_and_remain_discardable() {
    // only lunch turns leave barriers (their facts are missing until retried)
    let mut s = lunch_session();
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
fn deadline_passed_applies_nothing() {
    let mut s = Session::new("dl", Mode::Negotiation, "i", "mock");
    let text = "$1,150 delivered.";
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

