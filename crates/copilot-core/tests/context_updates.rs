//! Voice-context update regressions (PLAN v5, negotiation only): proposals are
//! never applied without confirmation; stale, superseded, failed and duplicate
//! updates can never change goals or publish a positive cue.

use copilot_core::context::*;
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
            Job::Context { .. } => {}
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
    confirm_all(&mut s);
    drain_evals(&mut s, None);
    assert_eq!(outcome(&s), Outcome::Proceed);
    s
}

fn ctx_req(uid: &str, seq: Option<u64>, text: &str) -> CtxAcceptReq {
    CtxAcceptReq {
        mode: Mode::Negotiation,
        source_id: "cy-bridge".into(),
        update_id: uid.into(),
        source_seq: seq,
        trigger: TRIGGER_THREE_WAVES.into(),
        text: text.into(),
        transcript_source: "simulated".into(),
        input_source: "simulated".into(),
        now_ms: 0,
    }
}

fn accept_ctx(s: &mut Session, uid: &str, text: &str) -> u64 {
    match s.accept_context(ctx_req(uid, None, text)).unwrap() {
        Accepted::New(n) => n,
        Accepted::Duplicate(_) => panic!("unexpected duplicate"),
    }
}

fn budget(amount: &str, quote: &str) -> RawCtxChange {
    RawCtxChange {
        field: CtxField::Budget,
        amount_text: Some(amount.into()),
        enum_value: EnumValue::NotApplicable,
        quantity_text: None,
        duration_unit: DurUnit::NotApplicable,
        quote: quote.into(),
    }
}
fn ctx_size(v: EnumValue, quote: &str) -> RawCtxChange {
    RawCtxChange {
        field: CtxField::Size,
        amount_text: None,
        enum_value: v,
        quantity_text: None,
        duration_unit: DurUnit::NotApplicable,
        quote: quote.into(),
    }
}
fn ctx_ret(n: &str, unit: DurUnit, quote: &str) -> RawCtxChange {
    RawCtxChange {
        field: CtxField::ReturnPeriod,
        amount_text: None,
        enum_value: EnumValue::NotApplicable,
        quantity_text: Some(n.into()),
        duration_unit: unit,
        quote: quote.into(),
    }
}
fn ext(changes: Vec<RawCtxChange>) -> CtxExtraction {
    CtxExtraction { changes }
}

/// Pop jobs until the context job for `seq` and start it.
fn start(s: &mut Session, seq: u64) -> CtxJob {
    loop {
        match s.pop_job() {
            Some(Job::Context { seq: q }) if q == seq => {
                return s.start_context(seq, 0, u64::MAX).expect("job")
            }
            Some(_) => continue,
            None => panic!("no context job"),
        }
    }
}

const BUDGET_UP: &str = "My budget is now $1,300 including delivery.";

#[test]
fn proposal_is_not_applied_until_confirmed_then_only_listed_goals_change() {
    let mut s = proceed_session();
    let g0 = s.goals.clone();
    let gv = s.goals_version;
    let seq = accept_ctx(&mut s, "u1", BUDGET_UP);
    assert_eq!(s.generation, seq, "updates share the session generation");
    let snap = s.snapshot("r");
    assert_eq!(
        snap.cue.unwrap().kind,
        CueKind::Clear,
        "LED clears on acceptance"
    );
    let job = start(&mut s, seq);
    s.commit_context(
        &job,
        Ok(ext(vec![budget("$1,300", "budget is now $1,300")])),
        0,
    );
    let c = s.context.clone().unwrap();
    assert_eq!(c.status, CtxStatus::Proposed);
    assert_eq!(s.goals, g0, "proposal never changes goals");
    drain_evals(&mut s, None);
    assert_eq!(outcome(&s), Outcome::Ask, "pending proposal blocks PROCEED");
    s.confirm_context(s.state_rev, "u1", c.proposal_version)
        .unwrap();
    assert_eq!(s.goals.budget_cents, 130_000);
    assert_eq!(s.goals.sizes, g0.sizes);
    assert_eq!(s.goals.firmness, g0.firmness);
    assert_eq!(s.goals.min_return_nights, g0.min_return_nights);
    assert_eq!(s.goals_version, gv + 1);
    assert_eq!(s.context.as_ref().unwrap().status, CtxStatus::Applied);
    assert_eq!(outcome(&s), Outcome::None, "invalidated until re-evaluated");
    drain_evals(&mut s, None);
    assert_eq!(
        outcome(&s),
        Outcome::Proceed,
        "re-evaluated against the new goals"
    );
    assert_eq!(s.generation, seq);
    // duplicate confirmation and discard-after-apply are rejected and change nothing
    let before = (s.goals.clone(), s.goals_version);
    let e = s
        .confirm_context(s.state_rev, "u1", c.proposal_version)
        .unwrap_err();
    assert_eq!(e.code, "context_already_applied");
    let e = s.discard_context(s.state_rev, "u1").unwrap_err();
    assert_eq!(e.code, "context_already_applied");
    assert_eq!((s.goals.clone(), s.goals_version), before);
}

#[test]
fn old_evaluation_finishing_after_acceptance_cannot_publish_positive_cue() {
    let mut s = new_session();
    let t = accept(&mut s, FULL, "t1");
    assert_eq!(apply(&mut s, t, &full_offer()), PhaseA::Applied);
    assert_eq!(evaluate(&mut s, Some(t), None), None);
    confirm_all(&mut s);
    let EvalStart::NeedsContextual(old) = (loop {
        match s.pop_job() {
            Some(Job::Evaluate { trigger }) => {
                break s.start_evaluation(trigger, None, 0, u64::MAX)
            }
            Some(_) => continue,
            None => panic!("no evaluation"),
        }
    }) else {
        panic!("expected a contextual check");
    };
    let _seq = accept_ctx(&mut s, "u1", BUDGET_UP);
    let c = s.commit_contextual(
        *old,
        ContextualResult::Noul(Ok(NoulVerdict::Clear { p_yes: 0.01 })),
        None,
        1,
        0,
    );
    assert_eq!(c, Commit::Stale);
    assert_ne!(outcome(&s), Outcome::Proceed);
}

#[test]
fn pending_gate_rejects_commit_even_when_state_rev_matches() {
    // the gate is re-checked under the commit lock, independent of state_rev
    let mut s = new_session();
    let t = accept(&mut s, FULL, "t1");
    assert_eq!(apply(&mut s, t, &full_offer()), PhaseA::Applied);
    assert_eq!(evaluate(&mut s, Some(t), None), None);
    confirm_all(&mut s);
    let EvalStart::NeedsContextual(old) = (loop {
        match s.pop_job() {
            Some(Job::Evaluate { trigger }) => {
                break s.start_evaluation(trigger, None, 0, u64::MAX)
            }
            Some(_) => continue,
            None => panic!("no evaluation"),
        }
    }) else {
        panic!("expected a contextual check");
    };
    let _seq = accept_ctx(&mut s, "u1", BUDGET_UP);
    let mut forged = *old;
    forged.state_rev = s.state_rev;
    let c = s.commit_contextual(
        forged,
        ContextualResult::Noul(Ok(NoulVerdict::Clear { p_yes: 0.01 })),
        None,
        1,
        0,
    );
    assert_eq!(c, Commit::Stale);
    assert_ne!(outcome(&s), Outcome::Proceed);
    // and the rules never request a contextual call while the update is open
    while let Some(j) = s.pop_job() {
        if let Job::Evaluate { trigger } = j {
            assert!(!matches!(
                s.start_evaluation(trigger, None, 0, u64::MAX),
                EvalStart::NeedsContextual(_)
            ));
        }
    }
    assert_eq!(
        outcome(&s),
        Outcome::None,
        "cue stays cleared while the update is extracted"
    );
}

#[test]
fn pause_priority_is_preserved_while_update_is_pending() {
    let mut s = new_session();
    let t = accept(&mut s, "Best I can do is $1,399 delivered.", "t1");
    let e = NegExtraction {
        turn_kind: TurnKind::NewOffer,
        amends_offer_id: None,
        terms: vec![total("1,399", "$1,399 delivered")],
    };
    assert_eq!(apply(&mut s, t, &e), PhaseA::Applied);
    let _ = evaluate(&mut s, Some(t), None);
    assert_eq!(outcome(&s), Outcome::Pause);
    let seq = accept_ctx(&mut s, "u1", BUDGET_UP);
    let job = start(&mut s, seq);
    s.commit_context(
        &job,
        Ok(ext(vec![budget("$1,300", "budget is now $1,300")])),
        0,
    );
    drain_evals(&mut s, None);
    assert_eq!(
        outcome(&s),
        Outcome::Pause,
        "known violation outranks the pending update"
    );
}

#[test]
fn goal_edit_during_extraction_makes_update_stale() {
    let mut s = proceed_session();
    let seq = accept_ctx(&mut s, "u1", BUDGET_UP);
    let job = start(&mut s, seq);
    let mut g = s.goals.clone();
    g.min_return_nights = 60;
    s.set_goals(s.goals_version, g.clone()).unwrap();
    s.commit_context(
        &job,
        Ok(ext(vec![budget("$1,300", "budget is now $1,300")])),
        0,
    );
    let c = s.context.clone().unwrap();
    assert_eq!(c.status, CtxStatus::Stale);
    assert_eq!(s.goals, g, "stale update changes nothing");
    let e = s.confirm_context(s.state_rev, "u1", 1).unwrap_err();
    assert_eq!(e.code, "stale_form");
    drain_evals(&mut s, None);
    assert_eq!(outcome(&s), Outcome::Ask);
    // retry re-extracts from the new base and can then be confirmed
    s.retry_context(s.state_rev, "u1", 0).unwrap();
    let job = start(&mut s, seq);
    assert_eq!(job.attempt, 2);
    s.commit_context(
        &job,
        Ok(ext(vec![budget("$1,300", "budget is now $1,300")])),
        0,
    );
    let c = s.context.clone().unwrap();
    assert_eq!(c.status, CtxStatus::Proposed);
    s.confirm_context(s.state_rev, "u1", c.proposal_version)
        .unwrap();
    assert_eq!(s.goals.budget_cents, 130_000);
    assert_eq!(s.goals.min_return_nights, 60);
}

#[test]
fn proposal_confirm_requires_current_goals_version() {
    let mut s = proceed_session();
    let seq = accept_ctx(&mut s, "u1", BUDGET_UP);
    let job = start(&mut s, seq);
    s.commit_context(
        &job,
        Ok(ext(vec![budget("$1,300", "budget is now $1,300")])),
        0,
    );
    let pv = s.context.as_ref().unwrap().proposal_version;
    let mut g = s.goals.clone();
    g.min_return_nights = 30;
    s.set_goals(s.goals_version, g.clone()).unwrap();
    // a form loaded before the edit carries the old state_rev
    let e = s.confirm_context(s.state_rev, "u1", pv).unwrap_err();
    assert_eq!(e.code, "stale_form");
    assert_eq!(s.goals, g);
    assert_eq!(s.context.as_ref().unwrap().status, CtxStatus::Stale);
}

#[test]
fn newer_update_supersedes_older_one() {
    let mut s = proceed_session();
    let a = accept_ctx(&mut s, "A", BUDGET_UP);
    let job_a = start(&mut s, a);
    let b = accept_ctx(&mut s, "B", "I want a king now.");
    assert!(b > a);
    assert_eq!(s.generation, b);
    // A's late result cannot touch B
    s.commit_context(
        &job_a,
        Ok(ext(vec![budget("$1,300", "budget is now $1,300")])),
        0,
    );
    let c = s.context.clone().unwrap();
    assert_eq!(c.update_id, "B");
    assert_eq!(c.status, CtxStatus::Pending);
    let job_b = start(&mut s, b);
    s.commit_context(&job_b, Ok(ext(vec![ctx_size(EnumValue::King, "king")])), 0);
    let pv = s.context.as_ref().unwrap().proposal_version;
    assert_eq!(
        s.confirm_context(s.state_rev, "A", 1).unwrap_err().code,
        "context_superseded"
    );
    assert_eq!(
        s.discard_context(s.state_rev, "A").unwrap_err().code,
        "context_superseded"
    );
    assert_eq!(s.context.as_ref().unwrap().status, CtxStatus::Proposed);
    s.confirm_context(s.state_rev, "B", pv).unwrap();
    assert_eq!(s.goals.sizes, vec![EnumValue::King]);
    assert_eq!(s.goals.budget_cents, 120_000, "A's budget never applied");
    let a_rec = s
        .context_history
        .iter()
        .find(|c| c.update_id == "A")
        .unwrap();
    assert_eq!(a_rec.status, CtxStatus::Superseded);
}

#[test]
fn failures_and_discards_preserve_goals() {
    let mut s = proceed_session();
    let g0 = (s.goals.clone(), s.goals_version);
    let seq = accept_ctx(&mut s, "u1", BUDGET_UP);
    let job = start(&mut s, seq);
    s.commit_context(&job, Err(ApiError::new("extraction_timeout", "x", true)), 0);
    assert_eq!(s.context.as_ref().unwrap().status, CtxStatus::Failed);
    drain_evals(&mut s, None);
    assert_eq!(outcome(&s), Outcome::Ask, "failed update blocks PROCEED");
    s.discard_context(s.state_rev, "u1").unwrap();
    assert_eq!(s.context.as_ref().unwrap().status, CtxStatus::Discarded);
    assert_eq!((s.goals.clone(), s.goals_version), g0);
    drain_evals(&mut s, None);
    assert_eq!(
        outcome(&s),
        Outcome::Proceed,
        "discarded update no longer blocks"
    );
    // no grounded change: failed, nothing proposed
    let seq = accept_ctx(&mut s, "u2", "Hmm, let me think about it.");
    let job = start(&mut s, seq);
    s.commit_context(&job, Ok(ext(vec![])), 0);
    let c = s.context.clone().unwrap();
    assert_eq!(c.status, CtxStatus::Failed);
    assert_eq!(c.error.unwrap().code, "context_no_change");
    assert_eq!((s.goals.clone(), s.goals_version), g0);
}

#[test]
fn duplicate_delivery_and_sequence_rules() {
    let mut s = proceed_session();
    let seq = match s.accept_context(ctx_req("u1", Some(5), BUDGET_UP)).unwrap() {
        Accepted::New(n) => n,
        _ => panic!(),
    };
    let rev = s.state_rev;
    assert_eq!(
        s.accept_context(ctx_req("u1", Some(5), BUDGET_UP)).unwrap(),
        Accepted::Duplicate(seq)
    );
    assert_eq!(s.state_rev, rev, "duplicate changes nothing");
    let e = s
        .accept_context(ctx_req("u1", Some(5), "My budget is now $1,500."))
        .unwrap_err();
    assert_eq!(e.code, "update_id_reused");
    let e = s
        .accept_context(ctx_req("u2", Some(5), BUDGET_UP))
        .unwrap_err();
    assert_eq!(e.code, "stale_update");
    // source_seq is shared with turns from the same source
    let e = s
        .accept_turn(AcceptReq {
            mode: Mode::Negotiation,
            source_id: "cy-bridge".into(),
            client_turn_id: "t9".into(),
            source_seq: Some(5),
            text: "Okay.".into(),
            transcript_source: "simulated".into(),
            speaker: None,
            input_source: "simulated".into(),
            now_ms: 0,
        })
        .unwrap_err();
    assert_eq!(e.code, "stale_turn");
    // confirmation applies at most once
    let job = start(&mut s, seq);
    s.commit_context(
        &job,
        Ok(ext(vec![budget("$1,300", "budget is now $1,300")])),
        0,
    );
    let pv = s.context.as_ref().unwrap().proposal_version;
    s.confirm_context(s.state_rev, "u1", pv).unwrap();
    let gv = s.goals_version;
    assert!(s.confirm_context(s.state_rev, "u1", pv).is_err());
    assert_eq!(s.goals_version, gv);
    // a duplicate delivery after APPLIED returns the stored result, not a new update
    assert_eq!(
        s.accept_context(ctx_req("u1", Some(5), BUDGET_UP)).unwrap(),
        Accepted::Duplicate(seq)
    );
    assert_eq!(s.goals_version, gv);
}

#[test]
fn lunch_and_unconfirmed_goals_are_rejected_before_extraction() {
    let mut l = Session::new("l", Mode::Lunch, "i", "mock");
    let mut r = ctx_req("u1", None, BUDGET_UP);
    r.mode = Mode::Lunch;
    assert_eq!(
        l.accept_context(r).unwrap_err().code,
        "context_update_unsupported"
    );
    assert!(l.queue.is_empty());
    let mut n = Session::new("n", Mode::Negotiation, "i", "mock");
    assert_eq!(
        n.accept_context(ctx_req("u1", None, BUDGET_UP))
            .unwrap_err()
            .code,
        "goals_unconfirmed"
    );
    assert!(n.queue.is_empty());
}

#[test]
fn only_absolute_grounded_values_are_proposed() {
    let g = NegGoals::default();
    let t = "My budget is now $1,300 including delivery.";
    let (ok, rej) =
        validate_extraction(&ext(vec![budget("$1,300", "budget is now $1,300")]), t, &g).unwrap();
    assert_eq!(ok.len(), 1);
    assert!(rej.is_empty());
    // relative amount
    let t = "I can go $100 more.";
    let (ok, rej) = validate_extraction(&ext(vec![budget("$100", "$100 more")]), t, &g).unwrap();
    assert!(ok.is_empty() && rej.len() == 1);
    // ungrounded quote / amount not in the clause
    let t = "My budget is $1,300.";
    let (ok, _) =
        validate_extraction(&ext(vec![budget("$1,400", "budget is $1,300")]), t, &g).unwrap();
    assert!(ok.is_empty());
    let (ok, _) =
        validate_extraction(&ext(vec![budget("$1,300", "budget is $9,999")]), t, &g).unwrap();
    assert!(ok.is_empty());
    // negated size
    let t = "I do not want a king.";
    let (ok, _) = validate_extraction(
        &ext(vec![ctx_size(EnumValue::King, "not want a king")]),
        t,
        &g,
    )
    .unwrap();
    assert!(ok.is_empty());
    // months are a range, not an exact value
    let t = "I need a 3 month return window.";
    let (ok, _) = validate_extraction(
        &ext(vec![ctx_ret("3", DurUnit::Months, "3 month return")]),
        t,
        &g,
    )
    .unwrap();
    assert!(ok.is_empty());
    let t = "I need a 60 night return.";
    let (ok, _) = validate_extraction(
        &ext(vec![ctx_ret("60", DurUnit::Nights, "60 night return")]),
        t,
        &g,
    )
    .unwrap();
    assert_eq!(ok[0].value, GoalValue::ReturnNights(60));
    // conflicting values for one field: nothing proposed for it
    let t = "Budget $1,300. Actually budget $1,250.";
    let (ok, rej) = validate_extraction(
        &ext(vec![
            budget("$1,300", "Budget $1,300"),
            budget("$1,250", "budget $1,250"),
        ]),
        t,
        &g,
    )
    .unwrap();
    assert!(ok.is_empty());
    assert_eq!(rej.len(), 1);
    // server-side cardinality
    let many = ext((0..7)
        .map(|_| budget("$1,300", "budget is now $1,300"))
        .collect());
    assert!(validate_extraction(&many, "My budget is now $1,300.", &g).is_err());
}

#[test]
fn provider_schema_has_no_array_bounds_and_matches_the_contract() {
    let s = context_schema();
    let txt = s.to_string();
    assert!(!txt.contains("maxItems") && !txt.contains("minItems"));
    let parsed: CtxExtraction = serde_json::from_value(serde_json::json!({"changes": [
        {"field":"budget","amount_text":"$1,300","enum_value":"not_applicable","quantity_text":null,
         "duration_unit":"not_applicable","quote":"budget is now $1,300"}]}))
    .unwrap();
    assert_eq!(parsed.changes[0].field, CtxField::Budget);
    // unknown fields are rejected
    assert!(serde_json::from_value::<CtxExtraction>(
        serde_json::json!({"changes": [], "apply": true})
    )
    .is_err());
}

// ---- review round 1 regressions -------------------------------------------

#[test]
fn rapid_updates_while_extraction_is_held_keep_queue_bounded_and_process_newest() {
    use copilot_core::limits::SESSION_JOB_QUEUE_MAX;
    let mut s = proceed_session();
    let first = accept_ctx(&mut s, "u0", BUDGET_UP);
    let held = start(&mut s, first); // extraction in flight
    let mut last = 0;
    for i in 1..=100 {
        last = accept_ctx(&mut s, &format!("u{i}"), BUDGET_UP);
        let ctx_jobs = s
            .queue
            .iter()
            .filter(|j| matches!(j, Job::Context { .. }))
            .count();
        assert_eq!(ctx_jobs, 1, "obsolete context jobs are coalesced");
        assert!(s.queue.len() <= SESSION_JOB_QUEUE_MAX);
        assert!(s.context_history.len() <= CONTEXT_HISTORY);
    }
    // the held extraction finishes late and changes nothing
    s.commit_context(
        &held,
        Ok(ext(vec![budget("$1,300", "budget is now $1,300")])),
        0,
    );
    assert_eq!(s.context.as_ref().unwrap().update_id, "u100");
    assert_eq!(s.context.as_ref().unwrap().status, CtxStatus::Pending);
    // the only queued context job is the newest update
    let jobs: Vec<Job> = s
        .queue
        .iter()
        .filter(|j| matches!(j, Job::Context { .. }))
        .cloned()
        .collect();
    assert_eq!(jobs, vec![Job::Context { seq: last }]);
    let job = start(&mut s, last);
    s.commit_context(
        &job,
        Ok(ext(vec![budget("$1,300", "budget is now $1,300")])),
        0,
    );
    assert_eq!(s.context.as_ref().unwrap().status, CtxStatus::Proposed);
    assert!(s
        .pop_job()
        .is_none_or(|j| !matches!(j, Job::Context { .. })));
}

#[test]
fn discard_and_retry_prune_queued_context_jobs() {
    let mut s = proceed_session();
    let _seq = accept_ctx(&mut s, "u1", BUDGET_UP);
    s.discard_context(s.state_rev, "u1").unwrap();
    assert!(!s.queue.iter().any(|j| matches!(j, Job::Context { .. })));
    let seq = accept_ctx(&mut s, "u2", BUDGET_UP);
    let job = start(&mut s, seq);
    s.commit_context(&job, Err(ApiError::new("extraction_timeout", "x", true)), 0);
    for _ in 0..5 {
        s.retry_context(s.state_rev, "u2", 0).unwrap();
        let n = s
            .queue
            .iter()
            .filter(|j| matches!(j, Job::Context { .. }))
            .count();
        assert_eq!(n, 1);
        // a retried pending update can be retried again only after it fails
        let job = start(&mut s, seq);
        s.commit_context(&job, Err(ApiError::new("extraction_timeout", "x", true)), 0);
    }
}

#[test]
fn total_job_queue_bound_is_shared_with_turns_and_evaluations() {
    use copilot_core::limits::SESSION_JOB_QUEUE_MAX;
    let mut s = proceed_session();
    while s.queue.len() < SESSION_JOB_QUEUE_MAX {
        let n = s.queue.len() as u64;
        s.queue.push_back(Job::Evaluate {
            trigger: Some(1000 + n),
        });
    }
    let e = s
        .accept_context(ctx_req("u1", None, BUDGET_UP))
        .unwrap_err();
    assert_eq!(e.code, "turn_queue_full");
    let e = s
        .accept_turn(AcceptReq {
            mode: Mode::Negotiation,
            source_id: "cy-bridge".into(),
            client_turn_id: "tq".into(),
            source_seq: None,
            text: "Okay.".into(),
            transcript_source: "simulated".into(),
            speaker: None,
            input_source: "simulated".into(),
            now_ms: 0,
        })
        .unwrap_err();
    assert_eq!(e.code, "turn_queue_full");
    assert_eq!(s.queue.len(), SESSION_JOB_QUEUE_MAX);
}

#[test]
fn conflicted_fields_stay_excluded_in_every_order_and_repetition() {
    let g = NegGoals::default();
    let t = "Budget $1,100. No, budget $1,300. Fine, budget $1,500. I want a king.";
    let b = |a: &str| budget(a, &format!("budget {a}"));
    let orders: Vec<Vec<&str>> = vec![
        vec!["$1,100", "$1,300", "$1,500"],
        vec!["$1,500", "$1,100", "$1,300"],
        vec!["$1,300", "$1,500", "$1,100"],
        vec!["$1,100", "$1,100", "$1,300", "$1,300", "$1,100"],
        vec!["$1,500", "$1,300", "$1,300", "$1,500", "$1,500"],
        vec!["$1,100", "$1,300", "$1,100", "$1,100"],
    ];
    for o in orders {
        let mut changes: Vec<RawCtxChange> = o.iter().map(|a| b(a)).collect();
        changes.insert(1, ctx_size(EnumValue::King, "I want a king"));
        // keep within the 6-change contract
        changes.truncate(MAX_CONTEXT_CHANGES);
        let (ok, rej) = validate_extraction(&ext(changes), t, &g).unwrap();
        assert!(
            ok.iter().all(|c| c.value.field() != CtxField::Budget),
            "order {o:?} reintroduced a conflicted budget: {ok:?}"
        );
        assert_eq!(ok.len(), 1, "independent size survives for {o:?}");
        assert_eq!(ok[0].value, GoalValue::Size(EnumValue::King));
        assert!(rej
            .iter()
            .any(|r| r.contains("conflicting values for budget")));
    }
    // identical repetitions are not a conflict
    let (ok, rej) =
        validate_extraction(&ext(vec![b("$1,300"), b("$1,300"), b("$1,300")]), t, &g).unwrap();
    assert_eq!(ok.len(), 1);
    assert!(rej.is_empty());
}

#[test]
fn context_generation_snapshots_carry_their_own_provenance_and_null_timings() {
    let mut s = Session::new("s1", Mode::Negotiation, "i1", "mock");
    s.set_goals(1, NegGoals::default()).unwrap();
    drain_evals(&mut s, None);
    let t = match s
        .accept_turn(AcceptReq {
            mode: Mode::Negotiation,
            source_id: "cy-bridge".into(),
            client_turn_id: "b1".into(),
            source_seq: None,
            text: FULL.into(),
            transcript_source: "deepgram".into(),
            speaker: None,
            input_source: "bridge".into(),
            now_ms: 0,
        })
        .unwrap()
    {
        Accepted::New(n) => n,
        _ => panic!(),
    };
    assert_eq!(apply(&mut s, t, &full_offer()), PhaseA::Applied);
    let _ = evaluate(&mut s, Some(t), None);
    let snap = s.snapshot("r");
    assert_eq!(snap.provenance.input_source, "bridge");
    assert!(snap.timings_ms.accept_to_publish.is_some());
    let mut r = ctx_req("ui-1", None, BUDGET_UP);
    r.source_id = "ui".into();
    r.transcript_source = "typed".into();
    r.input_source = "typed".into();
    r.trigger = TRIGGER_TYPED.into();
    let seq = match s.accept_context(r).unwrap() {
        Accepted::New(n) => n,
        _ => panic!(),
    };
    let check = |s: &Session| {
        let snap = s.snapshot("r");
        assert_eq!(snap.header.generation, seq);
        assert_eq!(snap.provenance.input_source, "typed");
        assert_eq!(snap.provenance.transcript_source.as_deref(), Some("typed"));
        assert_eq!(
            snap.timings_ms,
            Timings::default(),
            "unmeasured timings are null"
        );
    };
    check(&s);
    let job = start(&mut s, seq);
    s.commit_context(
        &job,
        Ok(ext(vec![budget("$1,300", "budget is now $1,300")])),
        0,
    );
    drain_evals(&mut s, None);
    check(&s);
    let pv = s.context.as_ref().unwrap().proposal_version;
    s.confirm_context(s.state_rev, "ui-1", pv).unwrap();
    confirm_all(&mut s);
    drain_evals(&mut s, None);
    assert_eq!(outcome(&s), Outcome::Proceed);
    check(&s);
}
