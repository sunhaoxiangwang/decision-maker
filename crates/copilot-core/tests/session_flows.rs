//! Deterministic session state-machine tests: N4 confirmation, barriers, retries,
//! supersession, confirmation races, persisted conditions, context limits.

use copilot_core::contextual::NoulVerdict;
use copilot_core::contract::*;
use copilot_core::negotiation::*;
use copilot_core::session::*;

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
fn total(amount: &str, quote: &str) -> RawTerm {
    RawTerm {
        amount_kind: AmountKindRaw::Total,
        total_covers: Covers::ItemDelivery,
        amount_text: Some(amount.into()),
        currency: Currency::Usd,
        predicate: Predicate::Is,
        ..blank(TermField::Amount, quote)
    }
}
fn size(q: &str) -> RawTerm {
    RawTerm {
        predicate: Predicate::Is,
        enum_value: EnumValue::Queen,
        ..blank(TermField::Size, q)
    }
}
fn firm(q: &str) -> RawTerm {
    RawTerm {
        predicate: Predicate::Is,
        enum_value: EnumValue::Firm,
        ..blank(TermField::Firmness, q)
    }
}
fn ret(n: &str, q: &str) -> RawTerm {
    RawTerm {
        predicate: Predicate::Is,
        quantity_text: Some(n.into()),
        duration_unit: DurUnit::Nights,
        ..blank(TermField::ReturnPeriod, q)
    }
}

const FULL: &str = "Best I can do is $1,150 delivered, queen, firm, with a 100-night return.";
fn full_offer() -> NegExtraction {
    NegExtraction {
        turn_kind: TurnKind::NewOffer,
        amends_offer_id: None,
        terms: vec![
            total("1,150", "$1,150 delivered"),
            size("queen"),
            firm("firm"),
            ret("100", "100-night return"),
        ],
    }
}
fn irrelevant() -> NegExtraction {
    NegExtraction {
        turn_kind: TurnKind::Irrelevant,
        amends_offer_id: None,
        terms: vec![],
    }
}

fn new_session() -> Session {
    let mut s = Session::new("s1", Mode::Negotiation, "i1", "mock");
    s.set_goals(1, NegGoals::default()).unwrap();
    drain_evals(&mut s, None);
    s
}

fn accept(s: &mut Session, text: &str, cid: &str) -> u64 {
    match s
        .accept_turn(AcceptReq {
            mode: Mode::Negotiation,
            source_id: "cy-bridge".into(),
            client_turn_id: cid.into(),
            source_seq: None,
            text: text.into(),
            transcript_source: "simulated".into(),
            speaker: None,
            input_source: "simulated".into(),
            now_ms: 0,
        })
        .unwrap()
    {
        Accepted::New(n) | Accepted::Duplicate(n) => n,
    }
}

/// Pop the turn job and apply its extraction (Phase A).
fn apply(s: &mut Session, seq: u64, ext: &NegExtraction) -> PhaseA {
    let job = s.pop_job();
    assert_eq!(job, Some(Job::Turn { seq }));
    let ctx = s.start_extraction(seq, 0, u64::MAX).expect("ctx");
    s.apply_negotiation(&ctx, ext, "scripted", 0)
}

/// Phase B with an optional contextual verdict.
fn evaluate(
    s: &mut Session,
    trigger: Option<u64>,
    verdict: Option<Result<NoulVerdict, ApiError>>,
) -> Option<Commit> {
    match s.start_evaluation(trigger, None, 0, u64::MAX) {
        EvalStart::NeedsContextual(ctx) => {
            let ctx = *ctx;
            let v = verdict.unwrap_or(Ok(NoulVerdict::Clear { p_yes: 0.05 }));
            Some(s.commit_contextual(ctx, ContextualResult::Noul(v), None, 1, 0))
        }
        _ => None,
    }
}

fn drain_evals(s: &mut Session, verdict: Option<NoulVerdict>) {
    while let Some(j) = s.pop_job() {
        match j {
            Job::Evaluate { trigger } => {
                evaluate(s, trigger, verdict.clone().map(Ok));
            }
            Job::Turn { .. } => panic!("unexpected turn job"),
        }
    }
}

fn outcome(s: &Session) -> Outcome {
    s.decision
        .as_ref()
        .map(|d| d.outcome)
        .unwrap_or(Outcome::None)
}

fn confirm_all(s: &mut Session) {
    let o = s.active.as_ref().unwrap();
    let items: Vec<(u64, u64)> = o
        .facts
        .iter()
        .filter(|f| !f.confirmed)
        .map(|f| (f.id, f.version))
        .collect();
    let id = o.id.clone();
    s.confirm_facts(s.state_rev, &id, &items, 0).unwrap();
}

fn proceed_session() -> Session {
    let mut s = new_session();
    let t = accept(&mut s, FULL, "t1");
    assert_eq!(apply(&mut s, t, &full_offer()), PhaseA::Applied);
    assert_eq!(evaluate(&mut s, Some(t), None), None);
    assert_eq!(outcome(&s), Outcome::Ask);
    assert_eq!(s.phase, Phase::AwaitingConfirmation);
    confirm_all(&mut s);
    drain_evals(&mut s, None);
    assert_eq!(outcome(&s), Outcome::Proceed);
    s
}

#[test]
fn n4_confirmation_then_proceed() {
    let s = proceed_session();
    assert_eq!(s.snapshot("r").cue.unwrap().name, "green");
}

#[test]
fn r9_new_turn_clears_cue_before_extraction() {
    let mut s = proceed_session();
    let _t = accept(&mut s, "$1,180 delivered.", "t2");
    let snap = s.snapshot("r");
    assert_eq!(snap.phase, Phase::Processing);
    assert_eq!(snap.cue.unwrap().kind, CueKind::Clear);
    assert!(snap.actionable);
}

#[test]
fn br1_failed_changed_offer_then_thanks_never_proceeds() {
    let mut s = proceed_session();
    let t2 = accept(&mut s, "New price: $1,180 delivered.", "t2");
    assert_eq!(s.pop_job(), Some(Job::Turn { seq: t2 }));
    s.start_extraction(t2, 0, u64::MAX).unwrap();
    s.fail_before_apply(t2, ApiError::new("extraction_timeout", "x", true), 0);
    assert!(s.barriers.contains(&t2));
    let t3 = accept(&mut s, "Thanks.", "t3");
    assert_eq!(apply(&mut s, t3, &irrelevant()), PhaseA::Applied);
    evaluate(&mut s, Some(t3), None);
    assert_eq!(outcome(&s), Outcome::Unavailable, "{:?}", s.decision);
    // discard -> re-evaluation -> PROCEED again (fresh contextual check)
    s.discard_turn(s.state_rev, t2).unwrap();
    drain_evals(&mut s, None);
    assert_eq!(outcome(&s), Outcome::Proceed);
}

#[test]
fn br2_new_offer_clears_earlier_barrier() {
    let mut s = proceed_session();
    let t2 = accept(&mut s, "something", "t2");
    s.pop_job();
    s.start_extraction(t2, 0, u64::MAX).unwrap();
    s.fail_before_apply(t2, ApiError::new("extraction_timeout", "x", true), 0);
    let t3 = accept(&mut s, FULL, "t3");
    assert_eq!(apply(&mut s, t3, &full_offer()), PhaseA::Applied);
    assert!(s.barriers.is_empty());
    evaluate(&mut s, Some(t3), None);
    assert_eq!(outcome(&s), Outcome::Ask);
}

#[test]
fn br3_retry_after_later_amendment_is_refused() {
    let mut s = proceed_session();
    let t2 = accept(&mut s, "x", "t2");
    s.pop_job();
    s.start_extraction(t2, 0, u64::MAX).unwrap();
    s.fail_before_apply(t2, ApiError::new("extraction_timeout", "x", true), 0);
    let oid = s.active.as_ref().unwrap().id.clone();
    let t3 = accept(&mut s, "and a 120-night return.", "t3");
    let amend = NegExtraction {
        turn_kind: TurnKind::Amendment,
        amends_offer_id: Some(oid),
        terms: vec![ret("120", "120-night return")],
    };
    assert_eq!(apply(&mut s, t3, &amend), PhaseA::Applied);
    evaluate(&mut s, Some(t3), None);
    assert_eq!(s.retry_turn(t2, 1).unwrap(), "retry_not_latest");
}

#[test]
fn rt1_retry_before_apply_creates_offer_once() {
    let mut s = new_session();
    let t = accept(&mut s, FULL, "t1");
    s.pop_job();
    s.start_extraction(t, 0, u64::MAX).unwrap();
    s.fail_before_apply(t, ApiError::new("extraction_timeout", "x", true), 0);
    assert_eq!(s.retry_turn(t, 2).unwrap(), "attempt_mismatch");
    assert_eq!(s.retry_turn(t, 1).unwrap(), "retry_started");
    assert_eq!(s.retry_turn(t, 2).unwrap(), "retry_in_progress");
    assert_eq!(apply(&mut s, t, &full_offer()), PhaseA::Applied);
    assert_eq!(s.next_offer_no, 2, "exactly one offer created");
    // a second apply of the same turn is dropped
    let ctx = ExtractCtx {
        seq: t,
        attempt: 2,
        mode: Mode::Negotiation,
        text: FULL.into(),
        active_offer_id: Some("o-1".into()),
        active_terms: String::new(),
        slot_versions: Default::default(),
        lunch_option_versions: [0; 3],
        lunch_prompt: String::new(),
        reextracted: false,
        deadline_ms: u64::MAX,
    };
    assert_eq!(
        s.apply_negotiation(&ctx, &full_offer(), "x", 0),
        PhaseA::Dropped
    );
    assert_eq!(s.next_offer_no, 2);
    evaluate(&mut s, Some(t), None);
    // duplicate returns the latest attempt
    let tr = s.turn_result(t, "r").unwrap();
    assert_eq!(tr.turn.attempt, 2);
    assert_eq!(tr.turn_outcome, TurnOutcome::Applied);
}

#[test]
fn rt2_retry_after_apply_skips_extraction() {
    let mut s = proceed_session();
    let t = accept(&mut s, "Thanks.", "t2");
    apply(&mut s, t, &irrelevant());
    evaluate(
        &mut s,
        Some(t),
        Some(Err(ApiError::new("decision_timeout", "x", true))),
    );
    assert_eq!(outcome(&s), Outcome::Unavailable);
    assert_eq!(s.turn(t).unwrap().status, TurnStatus::FailedAfterApply);
    assert!(s.barriers.is_empty(), "no barrier after apply");
    let offers = s.next_offer_no;
    assert_eq!(s.retry_turn(t, 1).unwrap(), "retry_started");
    assert_eq!(
        s.pop_job(),
        Some(Job::Evaluate { trigger: Some(t) }),
        "evaluation only, no extraction job"
    );
    evaluate(&mut s, Some(t), None);
    assert_eq!(outcome(&s), Outcome::Proceed);
    assert_eq!(s.next_offer_no, offers);
    assert_eq!(s.turn_result(t, "r").unwrap().turn.attempt, 2);
}

#[test]
fn r1_confirmation_vs_replacement_offer() {
    let mut s = new_session();
    let t = accept(&mut s, FULL, "t1");
    apply(&mut s, t, &full_offer());
    evaluate(&mut s, Some(t), None);
    let old_rev = s.state_rev;
    let old_offer = s.active.as_ref().unwrap().id.clone();
    let items: Vec<(u64, u64)> = s
        .active
        .as_ref()
        .unwrap()
        .facts
        .iter()
        .map(|f| (f.id, f.version))
        .collect();
    let t2 = accept(&mut s, "$1,180 delivered.", "t2");
    let ext = NegExtraction {
        turn_kind: TurnKind::NewOffer,
        amends_offer_id: None,
        terms: vec![total("1,180", "$1,180 delivered")],
    };
    apply(&mut s, t2, &ext);
    assert!(s.confirm_facts(old_rev, &old_offer, &items, 0).is_err());
    assert!(s
        .active
        .as_ref()
        .unwrap()
        .facts
        .iter()
        .all(|f| !f.confirmed));
}

#[test]
fn r2_confirmation_while_turn_pending_defers_evaluation() {
    let mut s = new_session();
    let t = accept(&mut s, FULL, "t1");
    apply(&mut s, t, &full_offer());
    evaluate(&mut s, Some(t), None);
    let _t2 = accept(&mut s, "hmm", "t2"); // queued, not extracted
    confirm_all(&mut s); // state_rev unchanged by acceptance -> accepted
                         // evaluation waits for the pending turn: nothing published
    let j = s
        .queue
        .iter()
        .find(|j| matches!(j, Job::Evaluate { .. }))
        .cloned()
        .unwrap();
    let Job::Evaluate { trigger } = j else {
        unreachable!()
    };
    assert!(matches!(
        s.start_evaluation(trigger, None, 0, u64::MAX),
        EvalStart::Skipped
    ));
    assert_ne!(outcome(&s), Outcome::Proceed);
}

#[test]
fn r5_historical_duplicate_has_no_snapshot() {
    let mut s = new_session();
    let t = accept(&mut s, FULL, "t1");
    apply(&mut s, t, &full_offer());
    evaluate(&mut s, Some(t), None);
    assert!(s.turn_result(t, "r").unwrap().snapshot.is_some());
    let t2 = accept(&mut s, "ok", "t2");
    apply(&mut s, t2, &irrelevant());
    evaluate(&mut s, Some(t2), None);
    assert!(s.turn_result(t, "r").unwrap().snapshot.is_none());
    // same id, same content -> duplicate; different content -> reused
    assert_eq!(
        s.accept_turn(AcceptReq {
            mode: Mode::Negotiation,
            source_id: "cy-bridge".into(),
            client_turn_id: "t1".into(),
            source_seq: Some(1),
            text: FULL.into(),
            transcript_source: "simulated".into(),
            speaker: None,
            input_source: "simulated".into(),
            now_ms: 0
        }),
        Ok(Accepted::Duplicate(t))
    );
    let e = s
        .accept_turn(AcceptReq {
            mode: Mode::Negotiation,
            source_id: "cy-bridge".into(),
            client_turn_id: "t1".into(),
            source_seq: Some(1),
            text: "different".into(),
            transcript_source: "simulated".into(),
            speaker: None,
            input_source: "simulated".into(),
            now_ms: 0,
        })
        .unwrap_err();
    assert_eq!(e.code, "turn_id_reused");
}

#[test]
fn r7_supersession_applies_facts_in_order_without_decision() {
    let mut s = new_session();
    let t1 = accept(&mut s, FULL, "t1");
    let t2 = accept(&mut s, "Thanks.", "t2");
    assert_eq!(apply(&mut s, t1, &full_offer()), PhaseA::Applied);
    assert_eq!(
        s.start_evaluation(Some(t1), None, 0, u64::MAX),
        EvalStart::Superseded
    );
    assert_eq!(
        Session::turn_outcome(s.turn(t1).unwrap()),
        TurnOutcome::AppliedSuperseded
    );
    assert!(s.active.is_some(), "older turn's facts applied");
    assert_eq!(apply(&mut s, t2, &irrelevant()), PhaseA::Applied);
    evaluate(&mut s, Some(t2), None);
    assert_eq!(outcome(&s), Outcome::Ask);
}

#[test]
fn t1_cancel_before_and_after_apply() {
    let mut s = new_session();
    let t = accept(&mut s, FULL, "t1");
    s.pop_job();
    let ctx = s.start_extraction(t, 0, u64::MAX).unwrap();
    assert_eq!(s.cancel_turn(t), "cancelled");
    assert_eq!(
        s.apply_negotiation(&ctx, &full_offer(), "x", 0),
        PhaseA::Dropped
    );
    assert!(s.active.is_none());
    let t2 = accept(&mut s, FULL, "t2");
    while s.queue.front() != Some(&Job::Turn { seq: t2 }) {
        s.pop_job();
    }
    apply(&mut s, t2, &full_offer());
    assert_eq!(s.cancel_turn(t2), "already_applied");
}

#[test]
fn k1_concern_persists_through_many_irrelevant_turns() {
    let mut s = proceed_session();
    let t = accept(&mut s, "Oh and there is a restocking fee.", "c");
    apply(&mut s, t, &irrelevant());
    evaluate(
        &mut s,
        Some(t),
        Some(Ok(NoulVerdict::Concern { p_yes: 0.85 })),
    );
    assert_eq!(outcome(&s), Outcome::Ask);
    for i in 0..11 {
        let ti = accept(&mut s, "ok", &format!("i{i}"));
        apply(&mut s, ti, &irrelevant());
        evaluate(
            &mut s,
            Some(ti),
            Some(Ok(NoulVerdict::Clear { p_yes: 0.01 })),
        );
        assert_ne!(outcome(&s), Outcome::Proceed, "turn {i}");
    }
}

#[test]
fn k2_context_limit_blocks_proceed() {
    let mut s = proceed_session();
    for i in 0..31 {
        let ti = accept(&mut s, "ok", &format!("i{i}"));
        apply(&mut s, ti, &irrelevant());
        evaluate(
            &mut s,
            Some(ti),
            Some(Ok(NoulVerdict::Clear { p_yes: 0.01 })),
        );
    }
    assert_eq!(outcome(&s), Outcome::Ask);
    let o = s.active.as_ref().unwrap();
    assert!(o
        .conditions
        .iter()
        .any(|c| c.kind == ConditionKind::ContextLimit));
}

#[test]
fn stale_observation_does_not_record_condition() {
    let mut s = new_session();
    let t = accept(&mut s, FULL, "t1");
    apply(&mut s, t, &full_offer());
    evaluate(&mut s, Some(t), None);
    confirm_all(&mut s);
    s.pop_job();
    let EvalStart::NeedsContextual(ctx) = s.start_evaluation(None, None, 0, u64::MAX) else {
        panic!()
    };
    let ctx = *ctx;
    // user changes state before the contextual answer is committed
    let gv = s.goals_version;
    s.set_goals(gv, NegGoals::default()).unwrap();
    let c = s.commit_contextual(
        ctx,
        ContextualResult::Noul(Ok(NoulVerdict::Concern { p_yes: 0.9 })),
        None,
        1,
        0,
    );
    assert_eq!(c, Commit::Stale);
    assert!(s.active.as_ref().unwrap().conditions.is_empty());
}

#[test]
fn hypothetical_new_offer_is_ambiguous() {
    let mut s = new_session();
    let t = accept(&mut s, "What if I paid $1,100 delivered?", "t1");
    let ext = NegExtraction {
        turn_kind: TurnKind::NewOffer,
        amends_offer_id: None,
        terms: vec![total("1,100", "$1,100 delivered")],
    };
    apply(&mut s, t, &ext);
    assert!(s.ambiguous.contains(&t));
    evaluate(&mut s, Some(t), None);
    assert_eq!(outcome(&s), Outcome::Ask);
}

#[test]
fn amendment_version_conflict_reextracts() {
    let mut s = new_session();
    let t = accept(&mut s, FULL, "t1");
    apply(&mut s, t, &full_offer());
    evaluate(&mut s, Some(t), None);
    let oid = s.active.as_ref().unwrap().id.clone();
    let t2 = accept(&mut s, "Make it a 120-night return.", "t2");
    s.pop_job();
    let ctx = s.start_extraction(t2, 0, u64::MAX).unwrap();
    // user confirms the return fact between snapshot and apply
    confirm_all(&mut s);
    let amend = NegExtraction {
        turn_kind: TurnKind::Amendment,
        amends_offer_id: Some(oid),
        terms: vec![ret("120", "120-night return")],
    };
    assert_eq!(s.apply_negotiation(&ctx, &amend, "x", 0), PhaseA::Reextract);
    let ctx2 = s.reextract_ctx(t2, u64::MAX).unwrap();
    assert_eq!(s.apply_negotiation(&ctx2, &amend, "x", 0), PhaseA::Applied);
}

#[test]
fn stale_turn_and_queue_limits() {
    let mut s = new_session();
    let mk = |cid: &str, seq: u64| AcceptReq {
        mode: Mode::Negotiation,
        source_id: "cy-bridge".into(),
        client_turn_id: cid.into(),
        source_seq: Some(seq),
        text: "x".into(),
        transcript_source: "simulated".into(),
        speaker: None,
        input_source: "simulated".into(),
        now_ms: 0,
    };
    s.accept_turn(mk("a", 5)).unwrap();
    assert_eq!(s.accept_turn(mk("b", 4)).unwrap_err().code, "stale_turn");
    s.accept_turn(mk("c", 6)).unwrap();
    s.accept_turn(mk("d", 7)).unwrap();
    s.accept_turn(mk("e", 8)).unwrap();
    assert_eq!(
        s.accept_turn(mk("f", 9)).unwrap_err().code,
        "turn_queue_full"
    );
    let long = "x".repeat(4001);
    assert_eq!(
        s.accept_turn(AcceptReq {
            text: long,
            ..mk("g", 10)
        })
        .unwrap_err()
        .code,
        "transcript_too_long"
    );
}

#[test]
fn retention_bounds_turn_records() {
    let mut s = new_session();
    for i in 0..200 {
        let t = accept(&mut s, &"🙂".repeat(1000), &format!("t{i}"));
        apply(&mut s, t, &irrelevant());
        evaluate(&mut s, Some(t), None);
    }
    assert!(s.turns.len() <= copilot_core::limits::TURN_RECORDS_PER_SESSION);
    assert!(s.transcript_bytes <= copilot_core::limits::TRANSCRIPT_BYTES_PER_SESSION);
    let snap = serde_json::to_vec(&s.snapshot("r")).unwrap();
    assert!(snap.len() <= copilot_core::limits::SNAPSHOT_MAX_BYTES);
}

#[test]
fn closed_session_rejects_and_drops() {
    let mut s = new_session();
    let t = accept(&mut s, FULL, "t1");
    s.pop_job();
    let ctx = s.start_extraction(t, 0, u64::MAX).unwrap();
    s.close();
    assert_eq!(
        s.apply_negotiation(&ctx, &full_offer(), "x", 0),
        PhaseA::Dropped
    );
    assert_eq!(s.snapshot("r").phase, Phase::Closed);
    let e = s
        .accept_turn(AcceptReq {
            mode: Mode::Negotiation,
            source_id: "cy-bridge".into(),
            client_turn_id: "z".into(),
            source_seq: None,
            text: "x".into(),
            transcript_source: "s".into(),
            speaker: None,
            input_source: "s".into(),
            now_ms: 0,
        })
        .unwrap_err();
    assert_eq!(e.code, "session_closed");
}
