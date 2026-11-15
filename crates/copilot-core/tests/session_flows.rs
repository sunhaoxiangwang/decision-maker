//! Negotiation session flows: free-text goals, the conversation the judge reads, and
//! the generic turn machinery (supersession, failures, cancellation, limits).

use copilot_core::context::{CtxAcceptReq, TRIGGER_THREE_WAVES};
use copilot_core::contract::*;
use copilot_core::judge::Judgment;
use copilot_core::session::*;

fn new_session() -> Session {
    Session::new("s1", Mode::Negotiation, "i1", "mock")
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
            speaker: Some("speaker1".into()),
            input_source: "simulated".into(),
            now_ms: 0,
        })
        .unwrap()
    {
        Accepted::New(n) | Accepted::Duplicate(n) => n,
    }
}

/// Phase A for one turn: it joins the conversation.
fn apply(s: &mut Session, seq: u64) {
    s.queue.retain(|j| *j != Job::Turn { seq });
    s.start_extraction(seq, 0, u64::MAX).unwrap();
    assert!(s.apply_conversation(seq));
}

/// Phase B: returns what the judge was asked, after committing `answer` (if any).
fn judge(s: &mut Session, trigger: Option<u64>, answer: Result<Judgment, ApiError>) -> Option<(String, String)> {
    let EvalStart::NeedsContextual(ctx) = s.start_evaluation(trigger, None, 0, u64::MAX) else {
        return None;
    };
    let ContextualRequest::Judge { goals, conversation } = ctx.request.clone() else {
        panic!("negotiation must ask the judge");
    };
    s.commit_contextual(*ctx, ContextualResult::Judge(answer), None, 1, 0);
    Some((goals, conversation))
}

fn says(outcome: Outcome, reason: &str) -> Result<Judgment, ApiError> {
    Ok(Judgment {
        outcome,
        reason: reason.into(),
    })
}

#[test]
fn turns_are_accepted_without_goals_and_judged_in_any_wording() {
    let mut s = new_session();
    // the band's first sentence: no goals set, nothing to confirm first
    let t = accept(&mut s, "I want a firm bed. For $100.", "t1");
    apply(&mut s, t);
    let (goals, conv) = judge(&mut s, Some(t), says(Outcome::None, "No offer yet.")).unwrap();
    assert_eq!(goals, "");
    assert_eq!(conv, "speaker1: I want a firm bed. For $100.");
    assert_eq!(s.snapshot("r").outcome, Outcome::None);

    let t2 = accept(&mut s, "This firm one is $140.", "t2");
    apply(&mut s, t2);
    let (_, conv) = judge(&mut s, Some(t2), says(Outcome::Pause, "Over your $100.")).unwrap();
    assert_eq!(conv.lines().count(), 2, "the judge reads the whole conversation");
    let snap = s.snapshot("r");
    assert_eq!(snap.outcome, Outcome::Pause);
    assert_eq!(snap.reason.as_deref(), Some("Over your $100."));
    assert_eq!(snap.provenance.decision_source, "mock_judge");
    assert_eq!(snap.cue.unwrap().name, "red");
}

#[test]
fn typed_goals_are_free_text_and_re_judge() {
    let mut s = new_session();
    let t = accept(&mut s, "$1,150 delivered.", "t1");
    apply(&mut s, t);
    judge(&mut s, Some(t), says(Outcome::Ask, "What is the trial?"));
    s.set_goals(1, "  queen, firm, under $1,200 delivered  ".into())
        .unwrap();
    assert_eq!(s.goals, "queen, firm, under $1,200 delivered");
    assert_eq!(s.snapshot("r").phase, Phase::Invalidated);
    let (goals, _) = judge(&mut s, None, says(Outcome::Proceed, "Within goals.")).unwrap();
    assert_eq!(goals, "queen, firm, under $1,200 delivered");
    assert_eq!(s.snapshot("r").outcome, Outcome::Proceed);
    // a form from before the change is stale
    assert_eq!(s.set_goals(1, "x".into()).unwrap_err().code, "stale_form");
    assert_eq!(
        s.set_goals(2, "y".repeat(4001)).unwrap_err().code,
        "goals_too_long"
    );
}

#[test]
fn spoken_goal_update_applies_at_once_and_owns_the_generation() {
    let mut s = new_session();
    let t = accept(&mut s, "$1,150 delivered.", "t1");
    apply(&mut s, t);
    judge(&mut s, Some(t), says(Outcome::Ask, "No goals yet."));
    let req = |uid: &str, seq: u64, text: &str| CtxAcceptReq {
        mode: Mode::Negotiation,
        source_id: "cy-bridge".into(),
        update_id: uid.into(),
        source_seq: Some(seq),
        trigger: TRIGGER_THREE_WAVES.into(),
        text: text.into(),
        transcript_source: "deepgram".into(),
        input_source: "bridge".into(),
        now_ms: 0,
    };
    let Accepted::New(u) = s.accept_context(req("u1", 2, "I want a firm bed for $100.")).unwrap() else {
        panic!("new update");
    };
    assert_eq!(s.generation, u);
    assert_eq!(s.goals, "I want a firm bed for $100.");
    s.accept_context(req("u2", 3, "And free delivery.")).unwrap();
    assert_eq!(s.goals, "I want a firm bed for $100.\nAnd free delivery.");
    assert!(matches!(
        s.accept_context(req("u2", 3, "And free delivery.")).unwrap(),
        Accepted::Duplicate(_)
    ));
    assert_eq!(
        s.accept_context(req("u3", 3, "late")).unwrap_err().code,
        "stale_update"
    );
    let (goals, conv) = judge(&mut s, None, says(Outcome::Pause, "Over $100.")).unwrap();
    assert!(goals.ends_with("And free delivery."));
    assert_eq!(conv, "speaker1: $1,150 delivered.");
    let r = s.context_result("cy-bridge", "u1", "r").unwrap();
    assert_eq!(r.cue, None);
    assert!(!r.actionable);
}

#[test]
fn new_turn_clears_cue_and_supersedes_the_older_judgement() {
    let mut s = new_session();
    let t1 = accept(&mut s, "$1,150.", "t1");
    apply(&mut s, t1);
    let t2 = accept(&mut s, "$1,180.", "t2");
    let snap = s.snapshot("r");
    assert_eq!(snap.phase, Phase::Processing);
    assert_eq!(snap.cue.unwrap().kind, CueKind::Clear);
    // t1's judgement must not publish while t2 is pending
    assert!(matches!(
        s.start_evaluation(Some(t1), None, 0, u64::MAX),
        EvalStart::Superseded
    ));
    apply(&mut s, t2);
    judge(&mut s, Some(t2), says(Outcome::Ask, "Trial?"));
    assert_eq!(s.snapshot("r").outcome, Outcome::Ask);
    assert_eq!(
        s.turn(t1).unwrap().status,
        TurnStatus::Completed { published: false }
    );
}

#[test]
fn judge_failure_is_unavailable_retryable_and_never_blocks_later_turns() {
    let mut s = new_session();
    let t = accept(&mut s, "$1,150.", "t1");
    apply(&mut s, t);
    judge(
        &mut s,
        Some(t),
        Err(ApiError::new("extraction_timeout", "no response", true)),
    );
    assert_eq!(s.snapshot("r").outcome, Outcome::Unavailable);
    assert_eq!(s.turn(t).unwrap().status, TurnStatus::FailedAfterApply);
    assert!(s.barriers.is_empty());
    assert_eq!(s.retry_turn(t, 1).unwrap(), "retry_started");
    judge(&mut s, Some(t), says(Outcome::Proceed, "Fine."));
    assert_eq!(s.snapshot("r").outcome, Outcome::Proceed);
    // a failure before apply (queue timeout) leaves no barrier either
    let t2 = accept(&mut s, "x", "t2");
    s.fail_before_apply(t2, ApiError::new("queue_timeout", "late", true), 0);
    assert!(s.barriers.is_empty());
    let t3 = accept(&mut s, "$1,100.", "t3");
    apply(&mut s, t3);
    let (_, conv) = judge(&mut s, Some(t3), says(Outcome::Proceed, "ok")).unwrap();
    assert!(!conv.contains(": x"), "an unapplied turn is not in the conversation");
}

#[test]
fn cancel_before_and_after_apply() {
    let mut s = new_session();
    let t = accept(&mut s, "$1,150.", "t1");
    s.pop_job();
    s.start_extraction(t, 0, u64::MAX).unwrap();
    assert_eq!(s.cancel_turn(t), "cancelled");
    assert!(!s.apply_conversation(t));
    let t2 = accept(&mut s, "$1,150.", "t2");
    apply(&mut s, t2);
    assert_eq!(s.cancel_turn(t2), "already_applied");
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
fn retention_and_judge_window_are_bounded() {
    let mut s = new_session();
    let mut last = None;
    for i in 0..200 {
        let t = accept(&mut s, &"🙂".repeat(1000), &format!("t{i}"));
        apply(&mut s, t);
        last = judge(&mut s, Some(t), says(Outcome::None, "small talk"));
    }
    assert!(s.turns.len() <= copilot_core::limits::TURN_RECORDS_PER_SESSION);
    assert!(s.transcript_bytes <= copilot_core::limits::TRANSCRIPT_BYTES_PER_SESSION);
    let snap = serde_json::to_vec(&s.snapshot("r")).unwrap();
    assert!(snap.len() <= copilot_core::limits::SNAPSHOT_MAX_BYTES);
    let (_, conv) = last.unwrap();
    assert!(conv.chars().count() <= copilot_core::limits::CONTEXT_MAX_CHARS);
    assert!(conv.lines().count() >= 1);
}

#[test]
fn closed_session_rejects_and_drops() {
    let mut s = new_session();
    let t = accept(&mut s, "$1,150.", "t1");
    s.pop_job();
    s.start_extraction(t, 0, u64::MAX).unwrap();
    s.close();
    assert!(!s.apply_conversation(t));
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
