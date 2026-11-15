//! Application state, session registry and the per-session worker (Rust pipeline).

use crate::memory::{FailPoint, Gate, MemoryHub};
use copilot_core::contract::*;
use copilot_core::limits::*;
use copilot_core::session::*;
use copilot_providers::{Decider, ExtractOutput, ExtractRequest, Extractor};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{watch, Notify, Semaphore};

pub struct SessionHandle {
    pub id: String,
    pub mode: Mode,
    pub state: Mutex<Session>,
    pub cue_tx: watch::Sender<Arc<CueSnapshot>>,
    pub wake: Notify,
    pub changed: Notify,
    pub worker: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl SessionHandle {
    /// Publish the current snapshot to pollers and wake waiters. Called after every
    /// mutation while still holding the session lock (non-blocking send).
    pub fn publish(&self, s: &Session) {
        if self.cue_tx.borrow().header.rev != s.rev
            || self.cue_tx.borrow().header.state_rev != s.state_rev
        {
            self.cue_tx.send_replace(Arc::new(s.snapshot("push")));
        }
        self.changed.notify_waiters();
        if !s.queue.is_empty() {
            self.wake.notify_one();
        }
    }
}

#[derive(Default)]
pub struct Registry {
    pub active: Option<Arc<SessionHandle>>,
    pub closed: VecDeque<Arc<SessionHandle>>,
}

pub struct Config {
    pub bind: String,
    pub data_dir: PathBuf,
    pub bridge_token: Option<String>,
    pub providers_label: String,
    /// COPILOT_VOICE_GOALS=auto: spoken goal updates apply without confirmation, and the
    /// bridge may start a negotiation session (otherwise only the companion UI does).
    pub voice_goals_auto: bool,
}

pub struct App {
    pub instance_id: String,
    pub csrf: String,
    pub cfg: Config,
    pub start: Instant,
    pub registry: Mutex<Registry>,
    pub extractor: Arc<dyn Extractor>,
    pub decider: Arc<dyn Decider>,
    pub memory: MemoryHub,
    pub slots: Arc<Semaphore>,
    pub polls: Arc<Semaphore>,
    pub requests: Arc<Semaphore>,
    pub req_counter: AtomicU64,
    pub jobs_running: AtomicU64,
    pub jobs_peak: AtomicU64,
}

impl App {
    pub fn new(
        cfg: Config,
        extractor: Arc<dyn Extractor>,
        decider: Arc<dyn Decider>,
        fail: Option<FailPoint>,
        gate: Option<Gate>,
    ) -> Arc<App> {
        Arc::new_cyclic(|weak: &std::sync::Weak<App>| {
            let w = weak.clone();
            let hook: crate::memory::Hook = Arc::new(move |version| {
                if let Some(app) = w.upgrade() {
                    app.on_memory_change(version);
                }
            });
            let memory = MemoryHub::start(cfg.data_dir.join("memory.json"), hook, fail, gate);
            App {
                instance_id: uuid::Uuid::new_v4().to_string(),
                csrf: uuid::Uuid::new_v4().simple().to_string(),
                cfg,
                start: Instant::now(),
                registry: Mutex::new(Registry::default()),
                extractor,
                decider,
                memory,
                slots: Arc::new(Semaphore::new(JOB_SLOTS)),
                polls: Arc::new(Semaphore::new(MAX_WAITING_POLLS)),
                requests: Arc::new(Semaphore::new(MAX_INFLIGHT_REQUESTS)),
                req_counter: AtomicU64::new(1),
                jobs_running: AtomicU64::new(0),
                jobs_peak: AtomicU64::new(0),
            }
        })
    }

    pub fn now_ms(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }

    pub fn request_id(&self) -> String {
        format!("req-{:06}", self.req_counter.fetch_add(1, Ordering::SeqCst))
    }

    pub fn active(&self) -> Option<Arc<SessionHandle>> {
        self.registry.lock().expect("registry").active.clone()
    }

    pub fn find(&self, id: &str) -> Option<Arc<SessionHandle>> {
        let r = self.registry.lock().expect("registry");
        r.active
            .iter()
            .chain(r.closed.iter())
            .find(|h| h.id == id)
            .cloned()
    }

    /// Called by the memory actor while holding the memory publication lock
    /// (lock order: memory publication -> session).
    fn on_memory_change(&self, version: u64) {
        if let Some(h) = self.active() {
            let mut s = h.state.lock().expect("session");
            s.memory_changed(version);
            h.publish(&s);
        }
    }

    /// Create a new active session; the previous one is closed.
    pub fn create_session(self: &Arc<Self>, mode: Mode) -> Arc<SessionHandle> {
        let id = uuid::Uuid::new_v4().to_string();
        let mut s = Session::new(&id, mode, &self.instance_id, &self.cfg.providers_label);
        s.auto_goals = self.cfg.voice_goals_auto;
        let snap = Arc::new(s.snapshot("init"));
        let (tx, _) = watch::channel(snap);
        let h = Arc::new(SessionHandle {
            id,
            mode,
            state: Mutex::new(s),
            cue_tx: tx,
            wake: Notify::new(),
            changed: Notify::new(),
            worker: Mutex::new(None),
        });
        let jh = tokio::spawn(worker(self.clone(), h.clone()));
        *h.worker.lock().expect("worker") = Some(jh);
        let mut r = self.registry.lock().expect("registry");
        if let Some(old) = r.active.take() {
            close_handle(&old);
            r.closed.push_back(old);
            while r.closed.len() > CLOSED_SESSIONS_KEPT {
                if let Some(x) = r.closed.pop_front() {
                    close_handle(&x);
                }
            }
        }
        r.active = Some(h.clone());
        h
    }

    pub fn close_active(&self) {
        let mut r = self.registry.lock().expect("registry");
        if let Some(old) = r.active.take() {
            close_handle(&old);
            r.closed.push_back(old);
            while r.closed.len() > CLOSED_SESSIONS_KEPT {
                r.closed.pop_front();
            }
        }
    }
}

pub fn close_handle(h: &SessionHandle) {
    {
        let mut s = h.state.lock().expect("session");
        s.close();
        h.publish(&s);
    }
    if let Some(j) = h.worker.lock().expect("worker").take() {
        j.abort();
    }
}

struct RunningGuard<'a>(&'a App);
impl Drop for RunningGuard<'_> {
    fn drop(&mut self) {
        self.0.jobs_running.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Per-session worker: processes turns strictly in order (PLAN v3 §4).
pub async fn worker(app: Arc<App>, h: Arc<SessionHandle>) {
    loop {
        let notified = h.wake.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        let job = {
            let mut s = h.state.lock().expect("session");
            if s.closed {
                return;
            }
            s.pop_job()
        };
        let Some(job) = job else {
            notified.await;
            continue;
        };
        // job slot owned until this job's work has fully finished
        let permit = match tokio::time::timeout(
            Duration::from_millis(SLOT_WAIT_MS),
            app.slots.clone().acquire_owned(),
        )
        .await
        {
            Ok(Ok(p)) => p,
            _ => {
                let err = ApiError::new("capacity_unavailable", "no job slot available", true);
                let mut s = h.state.lock().expect("session");
                match job {
                    Job::Turn { seq } => s.fail_before_apply(seq, err, app.now_ms()),
                    Job::Evaluate { trigger } => s.fail_after_apply(trigger, err, app.now_ms()),
                    Job::Context { seq } => s.fail_context(seq, err),
                }
                h.publish(&s);
                continue;
            }
        };
        let running = app.jobs_running.fetch_add(1, Ordering::SeqCst) + 1;
        app.jobs_peak.fetch_max(running, Ordering::SeqCst);
        let _guard = RunningGuard(&app);
        let deadline = tokio::time::Instant::now() + Duration::from_millis(JOB_DEADLINE_MS);
        // the same absolute deadline is re-checked under the session lock before any
        // fact application or publication
        let deadline_ms = app.now_ms() + JOB_DEADLINE_MS;
        match job {
            Job::Turn { seq } => {
                let r = tokio::time::timeout_at(deadline, process_turn(&app, &h, seq, deadline_ms))
                    .await;
                if r.is_err() {
                    let mut s = h.state.lock().expect("session");
                    let applied = s.turn(seq).is_some_and(|t| t.effect_applied);
                    let err = ApiError::new(
                        "job_deadline",
                        format!("turn processing exceeded {JOB_DEADLINE_MS} ms"),
                        true,
                    );
                    if applied {
                        s.fail_after_apply(Some(seq), err, app.now_ms());
                    } else {
                        s.fail_before_apply(seq, err, app.now_ms());
                    }
                    h.publish(&s);
                }
            }
            Job::Context { seq } => {
                let r =
                    tokio::time::timeout_at(deadline, process_context(&app, &h, seq, deadline_ms))
                        .await;
                if r.is_err() {
                    let mut s = h.state.lock().expect("session");
                    s.fail_context(
                        seq,
                        ApiError::new(
                            "job_deadline",
                            format!("context update exceeded {JOB_DEADLINE_MS} ms"),
                            true,
                        ),
                    );
                    h.publish(&s);
                }
            }
            Job::Evaluate { trigger } => {
                let r = tokio::time::timeout_at(
                    deadline,
                    process_eval(&app, &h, trigger, true, deadline_ms),
                )
                .await;
                if r.is_err() {
                    let mut s = h.state.lock().expect("session");
                    s.fail_after_apply(
                        trigger,
                        ApiError::new(
                            "job_deadline",
                            format!("evaluation exceeded {JOB_DEADLINE_MS} ms"),
                            true,
                        ),
                        app.now_ms(),
                    );
                    h.publish(&s);
                }
            }
        }
        drop(permit);
    }
}

async fn process_turn(app: &Arc<App>, h: &Arc<SessionHandle>, seq: u64, deadline_ms: u64) {
    let mut ctx = {
        let mut s = h.state.lock().expect("session");
        let c = s.start_extraction(seq, app.now_ms(), deadline_ms);
        h.publish(&s);
        match c {
            Some(c) => c,
            None => return,
        }
    };
    loop {
        let t0 = Instant::now();
        let req = ExtractRequest {
            mode: ctx.mode,
            transcript: ctx.text.clone(),
            active_offer_id: ctx.active_offer_id.clone(),
            active_terms: ctx.active_terms.clone(),
            lunch_options: ctx.lunch_prompt.clone(),
        };
        let res = app.extractor.extract(req).await;
        let ms = t0.elapsed().as_millis() as u64;
        let mut s = h.state.lock().expect("session");
        s.set_extract_ms(seq, ms);
        let now = app.now_ms();
        let outcome = match res {
            Err(e) => {
                s.fail_before_apply(seq, e, now);
                h.publish(&s);
                return;
            }
            Ok(ExtractOutput::Negotiation(ext)) if ctx.mode == Mode::Negotiation => {
                s.apply_negotiation(&ctx, &ext, app.extractor.name(), now)
            }
            Ok(ExtractOutput::Lunch(ext)) if ctx.mode == Mode::Lunch => {
                s.apply_lunch(&ctx, &ext, app.extractor.name(), now)
            }
            Ok(_) => PhaseA::Invalid(ApiError::new(
                "extraction_invalid_output",
                "wrong extraction mode",
                false,
            )),
        };
        match outcome {
            PhaseA::Applied => {
                h.publish(&s);
                break;
            }
            PhaseA::Dropped => {
                h.publish(&s);
                return;
            }
            PhaseA::Expired => {
                s.fail_before_apply(
                    seq,
                    ApiError::new(
                        "job_deadline",
                        format!("turn processing exceeded {JOB_DEADLINE_MS} ms"),
                        true,
                    ),
                    now,
                );
                h.publish(&s);
                return;
            }
            PhaseA::Invalid(e) => {
                s.fail_before_apply(seq, e, now);
                h.publish(&s);
                return;
            }
            PhaseA::Reextract => match s.reextract_ctx(seq, deadline_ms) {
                Some(c) => {
                    ctx = c;
                    continue;
                }
                None => return,
            },
        }
    }
    process_eval(app, h, Some(seq), true, deadline_ms).await;
}

async fn process_eval(
    app: &Arc<App>,
    h: &Arc<SessionHandle>,
    trigger: Option<u64>,
    allow_retry: bool,
    deadline_ms: u64,
) {
    let t0 = Instant::now();
    let is_lunch = h.mode == Mode::Lunch;
    // Every lunch publication path (including the rules-only "sole eligible"
    // selection) runs under the memory publication lock with the current view:
    // lock order memory publication -> session.
    let start = if is_lunch {
        let publ = app.memory.publ.lock().expect("memory lock");
        let view = publ.view();
        let mut s = h.state.lock().expect("session");
        let r = s.start_evaluation(trigger, Some(&view), app.now_ms(), deadline_ms);
        s.set_evaluate_ms(trigger, t0.elapsed().as_millis() as u64);
        h.publish(&s);
        r
    } else {
        let mut s = h.state.lock().expect("session");
        let r = s.start_evaluation(trigger, None, app.now_ms(), deadline_ms);
        s.set_evaluate_ms(trigger, t0.elapsed().as_millis() as u64);
        h.publish(&s);
        r
    };
    let ctx = match start {
        EvalStart::NeedsContextual(ctx) => *ctx,
        EvalStart::Expired => {
            fail_deadline(app, h, trigger);
            return;
        }
        _ => return,
    };
    let d0 = Instant::now();
    let result = match &ctx.request {
        ContextualRequest::Noul { terms, window } => {
            ContextualResult::Noul(app.decider.noul(terms.clone(), window.clone()).await)
        }
        ContextualRequest::Choice {
            soft_prefs,
            options,
        } => ContextualResult::Choice(
            app.decider
                .choice(soft_prefs.clone(), options.clone())
                .await,
        ),
    };
    let decide_ms = d0.elapsed().as_millis() as u64;
    let commit = if is_lunch {
        let publ = app.memory.publ.lock().expect("memory lock");
        let view = publ.view();
        let mut s = h.state.lock().expect("session");
        let c = s.commit_contextual(ctx, result, Some(&view), decide_ms, app.now_ms());
        h.publish(&s);
        c
    } else {
        let mut s = h.state.lock().expect("session");
        let c = s.commit_contextual(ctx, result, None, decide_ms, app.now_ms());
        h.publish(&s);
        c
    };
    match commit {
        Commit::Stale if allow_retry => {
            Box::pin(process_eval(app, h, trigger, false, deadline_ms)).await;
        }
        Commit::Stale => {
            let mut s = h.state.lock().expect("session");
            s.fail_after_apply(
                trigger,
                ApiError::new(
                    "state_contention",
                    "state kept changing during evaluation",
                    true,
                ),
                app.now_ms(),
            );
            h.publish(&s);
        }
        Commit::Expired => fail_deadline(app, h, trigger),
        _ => {}
    }
}

fn fail_deadline(app: &App, h: &SessionHandle, trigger: Option<u64>) {
    let mut s = h.state.lock().expect("session");
    s.fail_after_apply(
        trigger,
        ApiError::new(
            "job_deadline",
            format!("evaluation exceeded {JOB_DEADLINE_MS} ms"),
            true,
        ),
        app.now_ms(),
    );
    h.publish(&s);
}

/// Voice-context extraction: snapshot under the lock, call the provider with no
/// lock held, commit a proposal (never applied) under the lock.
async fn process_context(app: &Arc<App>, h: &Arc<SessionHandle>, seq: u64, deadline_ms: u64) {
    let job = {
        let mut s = h.state.lock().expect("session");
        let j = s.start_context(seq, app.now_ms(), deadline_ms);
        h.publish(&s);
        match j {
            Some(j) => j,
            None => return,
        }
    };
    let res = app
        .extractor
        .extract_context(job.text.clone(), job.goals_summary.clone())
        .await;
    let mut s = h.state.lock().expect("session");
    s.commit_context(&job, res, app.now_ms());
    h.publish(&s);
}
