//! Reference model of the bridge cue-acceptance rules (PLAN v4 §7).
//! Cy owns the real bridge; this pure model documents the contract, drives the
//! replay tool and backs the reference tests.

use crate::contract::{CueSnapshot, Mode};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    LedOff,
    Apply { rev: u64 },
    Resubmit { client_turn_id: String },
    Ignore(&'static str),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LookupResult {
    Found { server_seq: u64 },
    NotAccepted,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BridgeModel {
    pub boot_id: String,
    pub instance: Option<String>,
    pub session: Option<String>,
    pub mode: Option<Mode>,
    pub last_rev: u64,
    pub min_generation: u64,
    pub epoch: u64,
    pub mic_active: bool,
    pub awaiting: Option<(u64, String)>,
    pub resends: u32,
    pub blocked: bool,
}

impl BridgeModel {
    pub fn new(boot_id: &str) -> BridgeModel {
        BridgeModel {
            boot_id: boot_id.into(),
            ..Default::default()
        }
    }

    /// New instance/session/mode seen via session discovery.
    pub fn on_session(&mut self, instance: &str, session: &str, mode: Mode) -> Vec<Action> {
        let same = self.instance.as_deref() == Some(instance)
            && self.session.as_deref() == Some(session)
            && self.mode == Some(mode);
        if same {
            return vec![];
        }
        self.instance = Some(instance.into());
        self.session = Some(session.into());
        self.mode = Some(mode);
        self.last_rev = 0;
        self.min_generation = 0;
        self.awaiting = None;
        self.blocked = false;
        vec![Action::LedOff]
    }

    /// Start a capture: returns the client_turn_id for this epoch.
    pub fn capture_start(&mut self) -> (String, Vec<Action>) {
        self.epoch += 1;
        self.mic_active = true;
        let id = format!("{}-{}", self.boot_id, self.epoch);
        self.awaiting = Some((self.epoch, id.clone()));
        self.resends = 0;
        (id, vec![Action::LedOff])
    }

    pub fn mic_stopped(&mut self) {
        self.mic_active = false;
    }

    fn ids_match(&self, instance: &str, session: &str, mode: Mode) -> bool {
        self.instance.as_deref() == Some(instance)
            && self.session.as_deref() == Some(session)
            && self.mode == Some(mode)
    }

    /// Turn response or lookup result for `client_turn_id`.
    pub fn on_turn_response(
        &mut self,
        instance: &str,
        session: &str,
        mode: Mode,
        client_turn_id: &str,
        server_seq: Option<u64>,
    ) {
        if !self.ids_match(instance, session, mode) {
            return;
        }
        if let Some(s) = server_seq {
            self.min_generation = self.min_generation.max(s);
        }
        if self
            .awaiting
            .as_ref()
            .is_some_and(|(_, id)| id == client_turn_id)
            && server_seq.is_some()
        {
            self.awaiting = None;
            self.blocked = false;
        }
    }

    /// Submission timed out; caller performs a lookup and reports the result.
    pub fn on_lookup(
        &mut self,
        instance: &str,
        session: &str,
        mode: Mode,
        client_turn_id: &str,
        r: LookupResult,
    ) -> Vec<Action> {
        // a lookup result for another submission, session or instance must not
        // change epoch state (stale 404s and errors are ignored)
        if !self.ids_match(instance, session, mode)
            || self
                .awaiting
                .as_ref()
                .is_none_or(|(_, id)| id != client_turn_id)
        {
            return vec![Action::Ignore("stale lookup result")];
        }
        match r {
            LookupResult::Found { server_seq } => {
                self.on_turn_response(instance, session, mode, client_turn_id, Some(server_seq));
                vec![]
            }
            LookupResult::NotAccepted if self.resends < 2 => {
                self.resends += 1;
                vec![Action::Resubmit {
                    client_turn_id: client_turn_id.into(),
                }]
            }
            _ => {
                // fail closed: LED off, stay blocked until a newer epoch's turn is accepted
                self.blocked = true;
                vec![Action::LedOff]
            }
        }
    }

    pub fn should_apply(&self, s: &CueSnapshot) -> Result<(), &'static str> {
        if s.header.kind != "cue_snapshot" {
            return Err("not a cue snapshot");
        }
        if !s.actionable || s.cue.is_none() {
            return Err("not actionable");
        }
        let Some(mode) = s.header.mode else {
            return Err("no mode");
        };
        if !self.ids_match(
            &s.header.instance_id,
            s.header.session_id.as_deref().unwrap_or(""),
            mode,
        ) {
            return Err("instance/session/mode mismatch");
        }
        if self.mic_active {
            return Err("capturing");
        }
        if self.awaiting.is_some() || self.blocked {
            return Err("awaiting submission acknowledgement");
        }
        if s.header.generation < self.min_generation {
            return Err("older generation");
        }
        if s.header.rev <= self.last_rev {
            return Err("revision not newer");
        }
        Ok(())
    }

    pub fn on_snapshot(&mut self, s: &CueSnapshot) -> Action {
        match self.should_apply(s) {
            Ok(()) => {
                self.last_rev = s.header.rev;
                Action::Apply { rev: s.header.rev }
            }
            Err(why) => Action::Ignore(why),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::*;

    fn snap(
        instance: &str,
        session: &str,
        rev: u64,
        generation: u64,
        outcome: Outcome,
    ) -> CueSnapshot {
        CueSnapshot {
            header: Header {
                protocol: PROTOCOL.into(),
                request_id: "r".into(),
                instance_id: instance.into(),
                kind: "cue_snapshot".into(),
                session_id: Some(session.into()),
                mode: Some(Mode::Negotiation),
                rev,
                state_rev: 1,
                generation,
                error: None,
            },
            current: true,
            phase: Phase::Decided,
            outcome,
            selected_option: None,
            cue: Some(Cue::for_outcome(outcome, None)),
            actionable: true,
            reason: None,
            missing: vec![],
            conflicts: vec![],
            unconfirmed: vec![],
            conditions: vec![],
            barriers: vec![],
            truncated: false,
            provenance: Provenance {
                input_source: "bridge".into(),
                transcript_source: None,
                providers: "mock".into(),
                pipeline: "rust".into(),
                decision_source: "rules".into(),
            },
            timings_ms: Timings::default(),
        }
    }

    fn ready(m: &mut BridgeModel) {
        m.on_session("i1", "s1", Mode::Negotiation);
    }

    #[test]
    fn d6_delayed_old_session_proceed_rejected() {
        let mut m = BridgeModel::new("b");
        ready(&mut m);
        m.on_session("i1", "s2", Mode::Negotiation);
        assert!(matches!(
            m.on_snapshot(&snap("i1", "s1", 50, 9, Outcome::Proceed)),
            Action::Ignore(_)
        ));
        assert_eq!(
            m.on_snapshot(&snap("i1", "s2", 2, 0, Outcome::Ask)),
            Action::Apply { rev: 2 }
        );
    }

    #[test]
    fn d1_delayed_ack_of_a_during_b_keeps_awaiting() {
        let mut m = BridgeModel::new("b");
        ready(&mut m);
        let (a, _) = m.capture_start();
        m.mic_stopped();
        let (_b, _) = m.capture_start();
        m.mic_stopped();
        m.on_turn_response("i1", "s1", Mode::Negotiation, &a, Some(3));
        assert!(m.awaiting.is_some());
        assert!(matches!(
            m.on_snapshot(&snap("i1", "s1", 10, 3, Outcome::Proceed)),
            Action::Ignore(_)
        ));
    }

    #[test]
    fn d2_overlap_min_generation_never_regresses() {
        let mut m = BridgeModel::new("b");
        ready(&mut m);
        let (a, _) = m.capture_start();
        m.mic_stopped();
        let (b, _) = m.capture_start();
        m.mic_stopped();
        m.on_turn_response("i1", "s1", Mode::Negotiation, &b, Some(4));
        m.on_turn_response("i1", "s1", Mode::Negotiation, &a, Some(3));
        assert_eq!(m.min_generation, 4);
        assert!(m.awaiting.is_none());
        assert!(matches!(
            m.on_snapshot(&snap("i1", "s1", 10, 3, Outcome::Proceed)),
            Action::Ignore("older generation")
        ));
        assert_eq!(
            m.on_snapshot(&snap("i1", "s1", 11, 4, Outcome::Ask)),
            Action::Apply { rev: 11 }
        );
    }

    #[test]
    fn d3_d4_timeout_resync() {
        let mut m = BridgeModel::new("b");
        ready(&mut m);
        let (a, _) = m.capture_start();
        m.mic_stopped();
        let acts = m.on_lookup(
            "i1",
            "s1",
            Mode::Negotiation,
            &a,
            LookupResult::Found { server_seq: 7 },
        );
        assert!(acts.is_empty() && m.awaiting.is_none() && m.min_generation == 7);
        let (b, _) = m.capture_start();
        m.mic_stopped();
        assert!(matches!(
            m.on_lookup("i1", "s1", Mode::Negotiation, &b, LookupResult::NotAccepted)[0],
            Action::Resubmit { .. }
        ));
        m.on_lookup("i1", "s1", Mode::Negotiation, &b, LookupResult::NotAccepted);
        let acts = m.on_lookup("i1", "s1", Mode::Negotiation, &b, LookupResult::NotAccepted);
        assert_eq!(acts, vec![Action::LedOff]);
        assert!(matches!(
            m.on_snapshot(&snap("i1", "s1", 20, 8, Outcome::Proceed)),
            Action::Ignore(_)
        ));
    }

    #[test]
    fn d5_session_change_resets() {
        let mut m = BridgeModel::new("b");
        ready(&mut m);
        let _ = m.capture_start();
        assert_eq!(
            m.on_session("i2", "s1", Mode::Negotiation),
            vec![Action::LedOff]
        );
        assert!(m.awaiting.is_none() && m.last_rev == 0);
    }

    #[test]
    fn capturing_and_revision_rules() {
        let mut m = BridgeModel::new("b");
        ready(&mut m);
        assert_eq!(
            m.on_snapshot(&snap("i1", "s1", 5, 0, Outcome::Pause)),
            Action::Apply { rev: 5 }
        );
        assert!(matches!(
            m.on_snapshot(&snap("i1", "s1", 5, 0, Outcome::Proceed)),
            Action::Ignore(_)
        ));
        let _ = m.capture_start();
        assert!(matches!(
            m.on_snapshot(&snap("i1", "s1", 9, 0, Outcome::Proceed)),
            Action::Ignore("capturing")
        ));
    }
}
