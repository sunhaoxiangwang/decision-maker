//! Companion UI: Rust-rendered HTML, plain forms, no JavaScript, no inline styles
//! (PLAN v3 §10). Every state-changing form is bound to its session.

use crate::app::{App, SessionHandle};
use crate::memory::{Change, SaveResult};
use axum::extract::{Form, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use copilot_core::contract::{ApiError, Mode};
use copilot_core::distance::{DistUnit, Distance};
use copilot_core::lunch::{self, Allergen, AllergenStatus, Diet, LunchField, PrefKind, Strength};
use copilot_core::money::{fmt_cents, parse_amount_text};
use copilot_core::negotiation::{EnumValue, NegGoals};
use copilot_core::session::{
    AcceptReq, AmbiguousResolution, ConditionStatus, Correction, Session, TurnStatus,
};
use maud::{html, Markup, DOCTYPE};
use std::collections::HashMap;
use std::sync::Arc;

type F = Form<HashMap<String, String>>;
type Fields = HashMap<String, String>;

pub const CSS: &str = r#"
:root{--bg:#f7f7f5;--fg:#1d1d1b;--muted:#6b6b66;--card:#fff;--line:#ddd}
@media (prefers-color-scheme: dark){:root{--bg:#161615;--fg:#ecece8;--muted:#a3a39c;--card:#20201e;--line:#3a3a36}}
body{font:15px/1.45 system-ui,sans-serif;background:var(--bg);color:var(--fg);margin:0;padding:16px;max-width:1100px}
body.compact{padding:8px}
h1{font-size:20px;margin:0 0 8px}h2{font-size:16px;margin:0 0 8px}
.card{background:var(--card);border:1px solid var(--line);border-radius:8px;padding:12px;margin:0 0 12px}
.banner{padding:8px 12px;border-radius:6px;font-weight:600;margin:0 0 8px}
.mock{background:#ffe9a8;color:#5a4300}.real{background:#cfe8cf;color:#123d12}.unconf{background:#f6c9c9;color:#5c1010}
.row{display:flex;gap:12px;flex-wrap:wrap}.row>*{flex:1 1 320px}
table{border-collapse:collapse;width:100%}td,th{border-bottom:1px solid var(--line);padding:4px 6px;text-align:left;vertical-align:top;font-size:14px}
.muted{color:var(--muted)}
.swatch{display:inline-block;width:18px;height:18px;border-radius:50%;vertical-align:middle;border:1px solid #888;margin-right:6px}
.cue-green{background:#00FF00}.cue-red{background:#FF0000}.cue-amber{background:#FFB000}
.cue-purple{background:#8000FF}.cue-white{background:#FFFFFF}.cue-cyan{background:#00FFFF}.cue-off{background:#000000}
textarea{width:100%;min-height:60px;font:inherit}input,select,button{font:inherit}button{cursor:pointer}
.err{color:#b3261e;font-weight:600}.ok{color:#1b6e20;font-weight:600}
iframe{width:100%;height:340px;border:1px solid var(--line);border-radius:8px;background:var(--card)}
"#;

pub async fn css() -> Response {
    ([(header::CONTENT_TYPE, "text/css; charset=utf-8")], CSS).into_response()
}

fn page(title: &str, body: Markup) -> Html<String> {
    Html(
        html! {
            (DOCTYPE)
            html lang="en" {
                head { meta charset="utf-8"; meta name="viewport" content="width=device-width, initial-scale=1";
                    title { (title) } link rel="stylesheet" href="/static/app.css"; }
                body { (body) }
            }
        }
        .into_string(),
    )
}

fn provider_banner(app: &App) -> Markup {
    let l = app.cfg.providers_label.as_str();
    html! {
        @match l {
            "mock" => div.banner.mock { "MOCK PROVIDERS — deterministic stand-ins; not Gemini. Results are not evidence of provider integration." },
            "real" => div.banner.real { "Providers: Gemini extraction (generateContent) + Gemini Decisions (server-side)." },
            _ => div.banner.unconf { "Providers UNCONFIGURED — GEMINI_API_KEY not set; turns will be UNAVAILABLE." },
        }
    }
}

fn csrf(app: &App) -> Markup {
    html! { input type="hidden" name="csrf" value=(app.csrf); }
}

/// Hidden fields binding a form to its session and state revision.
fn bind(app: &App, s: &Session) -> Markup {
    html! { (csrf(app)) input type="hidden" name="session_id" value=(s.id); input type="hidden" name="state_rev" value=(s.state_rev); }
}

fn cue_swatch(name: &str) -> Markup {
    let class = match name {
        "green" | "red" | "amber" | "purple" | "white" | "cyan" | "off" => name,
        _ => "off",
    };
    html! { span class=(format!("swatch cue-{class}")) {} }
}

fn ms(v: Option<u64>) -> String {
    v.map(|v| v.to_string()).unwrap_or_else(|| "-".into())
}

fn status_inner(app: &App, h: &SessionHandle, seen: Option<u64>) -> Markup {
    let s = h.state.lock().expect("session");
    let snap = s.snapshot("ui");
    let latest = s.latest_turn().cloned();
    let pending_confirm = s
        .active
        .as_ref()
        .map(|o| o.facts.iter().filter(|f| !f.confirmed).count())
        .unwrap_or(0)
        + s.lunch
            .as_ref()
            .map(|l| {
                l.options
                    .iter()
                    .flat_map(|o| &o.facts)
                    .filter(|f| !f.confirmed)
                    .count()
            })
            .unwrap_or(0);
    let state_rev = s.state_rev;
    drop(s);
    html! {
        @if app.cfg.providers_label == "mock" { div.banner.mock { "MOCK PROVIDERS" } }
        @if let Some(t) = &latest { @if t.input_source != "bridge" { div.banner.mock { "SIMULATED / TYPED INPUT (" (t.input_source) ")" } } }
        p { b { "Mode: " } (h.mode.as_str()) " · rev " (snap.header.rev) " · state_rev " (state_rev) " · generation " (snap.header.generation) }
        p { b { "Phase: " } (format!("{:?}", snap.phase)) " · " b { "Outcome: " }
            @if let Some(c) = &snap.cue { (cue_swatch(&c.name)) (c.name) " " (c.pattern) " · " }
            b { (serde_json::to_value(snap.outcome).ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_default()) }
            @if let Some(o) = &snap.selected_option { " → option " b { (o) } } }
        @if let Some(r) = &snap.reason { p { b { "Reason: " } (r) } }
        @if let Some(e) = &snap.header.error { p.err { "Error " (e.code) ": " (e.message) } }
        @if !snap.missing.is_empty() { p { b { "Missing / unclear: " } (snap.missing.join("; ")) } }
        @if !snap.conflicts.is_empty() { p { b { "Conflicts: " } (snap.conflicts.join("; ")) } }
        @if !snap.unconfirmed.is_empty() { p { b { "Unconfirmed: " } (snap.unconfirmed.join("; ")) } }
        @if !snap.conditions.is_empty() { p { b { "Conditions: " } (snap.conditions.join("; ")) } }
        @if !snap.barriers.is_empty() { p.err { "Unprocessed turn(s): " (snap.barriers.iter().map(|b| b.to_string()).collect::<Vec<_>>().join(", ")) } }
        @if pending_confirm > 0 { p { a href="/confirm" target="_top" { (pending_confirm) " term(s) need confirmation — open" } } }
        @if seen.is_some_and(|v| v < state_rev) { p { a href="/" target="_top" { "State changed — load latest controls" } } }
        @if let Some(t) = &latest { p.muted { "Latest turn " (t.server_seq) " (" (t.input_source) ", " (t.transcript_source) "): “" (Session::excerpt_of(&t.text)) "” · status " (format!("{:?}", t.status)) " · attempt " (t.attempt) } }
        p.muted { "Server timings (ms): accept→first publish " (ms(snap.timings_ms.accept_to_publish))
            " · extract " (ms(snap.timings_ms.extract)) " · evaluate " (ms(snap.timings_ms.evaluate)) " · decide " (ms(snap.timings_ms.decide))
            " · generation e2e " (ms(snap.timings_ms.e2e)) " (human wait " (ms(snap.timings_ms.human_wait)) ", processing " (ms(snap.timings_ms.processing)) ")"
            " · providers=" (snap.provenance.providers) " pipeline=" (snap.provenance.pipeline) " decision_source=" (snap.provenance.decision_source) }
    }
}

#[derive(serde::Deserialize)]
pub struct SeenQ {
    pub seen: Option<u64>,
}

pub async fn status(State(app): State<Arc<App>>, Query(q): Query<SeenQ>) -> Html<String> {
    let body = match app.active() {
        Some(h) => status_inner(&app, &h, q.seen),
        None => html! { p.muted { "No active session." } },
    };
    Html(
        html! {
            (DOCTYPE) html { head { meta charset="utf-8"; meta http-equiv="refresh" content="1"; link rel="stylesheet" href="/static/app.css"; }
            body.compact { (body) } }
        }
        .into_string(),
    )
}

const CORRECT_FIELDS: &[(&str, &str)] = &[
    ("total", "total incl. delivery ($)"),
    ("size", "size (twin/full/queen/king/cal_king)"),
    ("firmness", "firmness (soft/medium/firm/extra_firm)"),
    ("return", "return nights"),
];

fn terms_table(s: &Session, app: &App) -> Markup {
    html! {
        @if let Some(o) = &s.active {
            p { b { "Active offer " (o.id) } " · " (s.active_offer_text()) }
            table { tr { th { "Term" } th { "Status" } th { "Evidence (excerpt)" } th { "Source" } }
                @for f in &o.facts { tr {
                    td { (f.stmt.describe()) }
                    td { @if f.confirmed { span.ok { "confirmed" } } @else { "unconfirmed" } }
                    td { "“" (f.excerpt) "”" @if f.excerpt_truncated { " (truncated)" } }
                    td.muted { @if let Some(seq) = f.source_seq { "turn " (seq) } " · " (f.extractor) " · v" (f.version) }
                } } }
            @for u in o.open_unresolved() {
                form method="post" action="/ui/unresolved/dismiss" { (bind(app, s))
                    input type="hidden" name="offer_id" value=(o.id); input type="hidden" name="id" value=(u.id); input type="hidden" name="version" value=(u.version);
                    span.err { "Unresolved " (u.field) ": " (u.reason) } " “" (u.excerpt) "” " button { "Not a term — dismiss" } }
            }
            @for c in &o.conditions { @if c.status == ConditionStatus::Unresolved {
                form method="post" action="/ui/condition/resolve" { (bind(app, s))
                    input type="hidden" name="offer_id" value=(o.id);
                    input type="hidden" name="cond_id" value=(c.id); input type="hidden" name="version" value=(c.version);
                    "Condition: " (c.note) " (turns " (c.covers.0) "–" (c.covers.1) ") " button { "Reviewed — no extra terms" } }
            } }
            form method="post" action="/ui/correct" { (bind(app, s)) input type="hidden" name="offer_id" value=(o.id);
                "Correct a value: " select name="field" { @for (k, label) in CORRECT_FIELDS { option value=(k) { (label) } } }
                " " input name="value" size="10"; " " button { "Save correction (confirmed)" } }
        } @else { p.muted { "No offer yet." } }
        @if let Some(e) = &s.eval_failed {
            form method="post" action="/ui/eval/retry" { (bind(app, s)) span.err { "Evaluation failed: " (e.code) " " } button { "Retry evaluation" } }
        }
    }
}

fn turn_controls(s: &Session, app: &App) -> Markup {
    html! {
        @for t in s.turns.iter().rev().take(12) {
            tr { td { (t.server_seq) } td { (t.input_source) } td { (Session::excerpt_of(&t.text)) }
                td { (format!("{:?}", t.status)) @if let Some(e) = &t.effect { " · " (e) } @if let Some(e) = &t.error { br; span.err { (e.code) } }
                     @for r in &t.rejected_terms { br; span.muted { "not applied: " (r) } } }
                td {
                    @if matches!(t.status, TurnStatus::FailedBeforeApply | TurnStatus::FailedAfterApply) {
                        form method="post" action="/ui/turn/retry" { (bind(app, s)) input type="hidden" name="seq" value=(t.server_seq);
                            input type="hidden" name="attempt" value=(t.attempt); button { "Retry" } }
                    }
                    @if t.status == TurnStatus::FailedBeforeApply {
                        form method="post" action="/ui/turn/discard" { (bind(app, s)) input type="hidden" name="seq" value=(t.server_seq); button { "Discard" } }
                    }
                    @if s.ambiguous.contains(&t.server_seq) {
                        form method="post" action="/ui/ambiguous/resolve" { (bind(app, s)) input type="hidden" name="seq" value=(t.server_seq);
                            button name="how" value="ignore" { "Ignore" } " "
                            @if !t.pending_terms.is_empty() {
                                button name="how" value="new_offer" { "Treat as new offer" } " "
                                button name="how" value="amendment" { "Treat as amendment" }
                            } }
                    }
                }
            }
        }
    }
}

fn goals_form(s: &Session, app: &App) -> Markup {
    let g = &s.goals;
    html! {
        form method="post" action="/ui/goals" { (bind(app, s)) input type="hidden" name="goals_version" value=(s.goals_version);
            "Size " select name="size" { @for v in ["twin","full","queen","king","cal_king"] { option value=(v) selected[g.sizes.first().map(|x| x.label().replace(' ', "_")) == Some(v.to_string())] { (v) } } }
            " Firmness " select name="firmness" { @for v in ["soft","medium","firm","extra_firm"] { option value=(v) selected[g.firmness.first().map(|x| x.label().replace(' ', "_")) == Some(v.to_string())] { (v) } } }
            " Budget incl. delivery $" input name="budget" size="6" value=(g.budget_cents / 100);
            " Min return nights " input name="nights" size="4" value=(g.min_return_nights);
            " " button { @if s.goals_confirmed { "Update and confirm goals" } @else { "Confirm goals" } }
            @if s.goals_confirmed { span.ok { " confirmed: " (g.sizes.iter().map(|v| v.label()).collect::<Vec<_>>().join("/")) ", " (g.firmness.iter().map(|v| v.label()).collect::<Vec<_>>().join("/")) ", ≤ " (fmt_cents(g.budget_cents)) " incl. delivery, ≥ " (g.min_return_nights) " nights" } }
        }
    }
}

fn lunch_setup(s: &Session, app: &App) -> Markup {
    let Some(l) = &s.lunch else { return html! {} };
    html! {
        form method="post" action="/ui/lunch/fixture" { (bind(app, s)) button { "Load demo options (user-entered fixture)" } }
        form method="post" action="/ui/lunch/setup" { (bind(app, s))
            @for (i, o) in l.options.iter().enumerate() { (o.letter) " " input name=(format!("name{i}")) value=(o.name) size="14"; " " }
            br; "Date " (l.date) " (" (l.timezone) ") window "
            input name="win_start" size="5" value=(lunch::hhmm(l.window.0)); "–" input name="win_end" size="5" value=(lunch::hhmm(l.window.1));
            " " button { "Save and confirm setup" }
            @if l.setup_confirmed { span.ok { " setup confirmed" } }
            p.muted { "Renaming an option starts a new option: its previous facts are removed." } }
        table { tr { th { "Option" } th { "Facts (status · source)" } }
            @for o in &l.options { tr { td { b { (o.letter) } " " (o.name) } td {
                @for f in &o.facts { (f.field.describe()) " · " @if f.confirmed { span.ok { "confirmed" } } @else { "unconfirmed" } " · " span.muted { (f.provenance) } @if !f.excerpt.is_empty() { span.muted { " “" (f.excerpt) "”" } } br; }
            } } } }
        @for u in l.unresolved.iter().filter(|u| u.open) {
            form method="post" action="/ui/lunch/unresolved/dismiss" { (bind(app, s)) input type="hidden" name="id" value=(u.id); input type="hidden" name="version" value=(u.version);
                span.err { "Unresolved " (u.option.map(|i| lunch::LETTERS[i]).unwrap_or("?")) " " (u.field) ": " (u.reason) } " “" (u.excerpt) "” " button { "Dismiss" } }
        }
        form method="post" action="/ui/lunch/fact" { (bind(app, s))
            "Set fact (replaces that kind of fact): option " select name="opt" { option { "A" } option { "B" } option { "C" } }
            " " select name="kind" { option value="peanut_free" { "peanut-free" } option value="peanut_contains" { "contains peanuts" }
                option value="distance" { "distance (mi)" } option value="hours" { "hours HH:MM-HH:MM" } option value="veg_yes" { "vegetarian options yes" } option value="veg_no" { "vegetarian options no" } }
            " value " input name="value" size="10"; " " button { "Save (user-entered, confirmed)" } }
    }
}

fn memory_section(app: &App) -> Markup {
    let p = app.memory.publ.lock().expect("memory").clone();
    html! {
        @if let Some(r) = &p.recovery {
            div.banner.unconf { "Preference memory needs recovery: " (r) ". Lunch decisions are blocked." }
            form method="post" action="/ui/memory/recheck" { (csrf(app)) button { "Re-check file" } }
            @if let Some(fp) = &p.fingerprint {
                form method="post" action="/ui/memory/new_profile" { (csrf(app)) input type="hidden" name="fingerprint" value=(fp);
                    "Type START NEW PROFILE to replace it (the bad file is kept): " input name="confirm_text" size="20"; button { "Start new profile" } }
            }
        }
        @if p.pending_change { p.muted { "A preference change is being saved…" } }
        p { "Confirmed preferences (memory v" (p.doc.memory_version) "):" }
        table { @for pr in &p.doc.prefs { tr { td { (pr.describe()) } td.muted { (pr.provenance) } td {
            form method="post" action="/ui/prefs/delete" { (csrf(app)) input type="hidden" name="memory_publication" value=(p.publication);
                input type="hidden" name="id" value=(pr.id); input type="hidden" name="version" value=(pr.version); button { "Delete" } } } } } }
        form method="post" action="/ui/prefs/add" { (csrf(app)) input type="hidden" name="memory_publication" value=(p.publication);
            "Add confirmed preference: " select name="kind" { option value="allergen_peanut" { "peanut allergy (hard)" } option value="max_walk" { "max walk miles (hard)" }
                option value="prefer_veg" { "prefer vegetarian (soft)" } option value="avoid_veg" { "avoid vegetarian (soft)" } }
            " value " input name="value" size="6"; " " button { "Save as confirmed preference" } }
    }
}

pub async fn index(State(app): State<Arc<App>>) -> Html<String> {
    let active = app.active();
    let body = html! {
        h1 { "Vexo Conversation Copilot" }
        (provider_banner(&app))
        div.card { h2 { "Session" }
            form method="post" action="/ui/session" { (csrf(&app))
                select name="mode" { option value="negotiation" { "negotiation" } option value="lunch" { "lunch" } }
                " " button { "Start new session" } }
            @if let Some(h) = &active {
                @let s = h.state.lock().expect("session").clone();
                p.muted { "Active session " (h.id) " (" (h.mode.as_str()) ")" }
                form method="post" action="/ui/close" { (bind(&app, &s)) button { "Close session" } } } }
        @if let Some(h) = &active {
            @let s = h.state.lock().expect("session").clone();
            div.card { h2 { "Live status (refreshes every second)" }
                iframe src=(format!("/ui/status?seen={}", s.state_rev)) title="status" {} }
            div.row {
                div.card { h2 { "Goals" }
                    @if h.mode == Mode::Negotiation { (goals_form(&s, &app)) } @else { (lunch_setup(&s, &app)) } }
                div.card { h2 { "Typed turn (simulated input)" }
                    form method="post" action="/ui/turn" { (bind(&app, &s)) input type="hidden" name="nonce" value=(uuid::Uuid::new_v4().simple().to_string());
                        textarea name="text" placeholder="e.g. Best I can do is $1,150 delivered, queen, firm, with a 100-night return." {}
                        button { "Submit turn" } }
                    p.muted { "Recording/transcription disclosure applies to the band path (Cy's bridge); typed turns are labelled simulated." } }
            }
            @if h.mode == Mode::Negotiation { div.card { h2 { "Terms " a href="/confirm" { "(confirm)" } } (terms_table(&s, &app)) } }
            @if h.mode == Mode::Lunch { div.card { h2 { "Preferences (confirmed memory)" } (memory_section(&app)) } }
            div.card { h2 { "Turns" } table { tr { th { "#" } th { "input" } th { "text" } th { "status" } th { "" } } (turn_controls(&s, &app)) } }
        }
    };
    page("Vexo Copilot", body)
}

pub async fn confirm_page(State(app): State<Arc<App>>) -> Html<String> {
    page("Confirm terms", confirm_body(&app))
}

fn confirm_body(app: &App) -> Markup {
    let Some(h) = app.active() else {
        return html! { p { "No active session. " a href="/" { "Back" } } };
    };
    let s = h.state.lock().expect("session").clone();
    html! {
        h1 { "Confirm extracted terms" } (provider_banner(app))
        @if let Some(o) = &s.active {
            @let pending: Vec<_> = o.facts.iter().filter(|f| !f.confirmed).collect();
            @if pending.is_empty() { p { "Nothing to confirm for offer " (o.id) "." } } @else {
                form method="post" action="/ui/confirm" { (bind(app, &s)) input type="hidden" name="offer_id" value=(o.id);
                    input type="hidden" name="items" value=(pending.iter().map(|f| format!("{}:{}", f.id, f.version)).collect::<Vec<_>>().join(","));
                    table { tr { th { "Term" } th { "Evidence" } th { "Source" } }
                        @for f in &pending { tr { td { (f.stmt.describe()) } td { "“" (f.excerpt) "”" } td.muted { "turn " (f.source_seq.unwrap_or(0)) " · " (f.extractor) } } } }
                    button { "Confirm all shown (" (pending.len()) ")" } }
            }
            @if !o.open_unresolved().is_empty() { p.err { "Open unresolved interpretations still block a recommendation; correct or dismiss them on the main page." } }
        }
        @if let Some(l) = &s.lunch {
            @let pending: Vec<_> = l.options.iter().flat_map(|o| o.facts.iter().filter(|f| !f.confirmed).map(move |f| (o, f))).collect();
            @if !pending.is_empty() {
                form method="post" action="/ui/lunch/confirm" { (bind(app, &s))
                    input type="hidden" name="items" value=(pending.iter().map(|(o, f)| format!("{}:{}:{}", o.letter, f.id, f.version)).collect::<Vec<_>>().join(","));
                    table { @for (o, f) in &pending { tr { td { (o.letter) " " (o.name) } td { (f.field.describe()) } td { "“" (f.excerpt) "”" } } } }
                    button { "Confirm all shown (" (pending.len()) ")" } }
            }
        }
        p { a href="/" { "Back to the app" } }
    }
}

// ---------------------------------------------------------------------------
// POST handlers
// ---------------------------------------------------------------------------

fn check_post(app: &App, headers: &HeaderMap, f: &Fields) -> Result<(), Box<Response>> {
    if f.get("csrf").map(String::as_str) != Some(app.csrf.as_str()) {
        return Err(Box::new(
            (StatusCode::FORBIDDEN, "missing or invalid form token").into_response(),
        ));
    }
    if let Some(o) = headers.get("origin").and_then(|v| v.to_str().ok()) {
        let p = port(&app.cfg.bind);
        let ok = o == format!("http://127.0.0.1:{p}") || o == format!("http://localhost:{p}");
        if !ok {
            return Err(Box::new(
                (StatusCode::FORBIDDEN, "cross-origin form rejected").into_response(),
            ));
        }
    }
    if let Some(s) = headers.get("sec-fetch-site").and_then(|v| v.to_str().ok()) {
        if s != "same-origin" && s != "none" {
            return Err(Box::new(
                (StatusCode::FORBIDDEN, "cross-site form rejected").into_response(),
            ));
        }
    }
    Ok(())
}

/// The form must belong to the currently active, open session.
fn bound_session(
    app: &App,
    headers: &HeaderMap,
    f: &Fields,
) -> Result<Arc<SessionHandle>, Box<Response>> {
    check_post(app, headers, f)?;
    let sid = f.get("session_id").map(String::as_str).unwrap_or("");
    match app.active() {
        Some(h) if !sid.is_empty() && h.id == sid && !h.state.lock().expect("session").closed => {
            Ok(h)
        }
        _ => Err(Box::new(stale_page(
            app,
            "This form belongs to a different or closed session — nothing changed.",
        ))),
    }
}

pub fn port(bind: &str) -> &str {
    bind.rsplit(':').next().unwrap_or("8787")
}

fn back() -> Response {
    Redirect::to("/").into_response()
}

fn stale_page(app: &App, msg: &str) -> Response {
    (
        StatusCode::CONFLICT,
        page(
            "Out of date",
            html! {
                h1 { "Out of date — nothing changed" } p.err { (msg) }
                (confirm_body(app))
            },
        ),
    )
        .into_response()
}

fn error_page(status: StatusCode, msg: &str) -> Response {
    (
        status,
        page(
            "Not applied",
            html! { p.err { (msg) } a href="/" { "Back" } },
        ),
    )
        .into_response()
}

fn num<T: std::str::FromStr>(f: &Fields, k: &str) -> Option<T> {
    f.get(k).and_then(|v| v.trim().parse().ok())
}

/// Run a session mutation under the session lock and publish.
fn mutate(
    app: &App,
    h: &SessionHandle,
    op: impl FnOnce(&mut Session) -> Result<(), ApiError>,
) -> Response {
    let r = {
        let mut s = h.state.lock().expect("session");
        let r = op(&mut s);
        h.publish(&s);
        r
    };
    match r {
        Ok(()) => back(),
        Err(e) if e.code == "stale_form" => stale_page(app, &e.message),
        Err(e) => error_page(
            StatusCode::UNPROCESSABLE_ENTITY,
            &format!("{}: {}", e.code, e.message),
        ),
    }
}

macro_rules! guard {
    ($app:expr, $h:expr, $f:expr) => {
        if let Err(r) = check_post(&$app, &$h, &$f) {
            return *r;
        }
    };
}

macro_rules! session {
    ($app:expr, $h:expr, $f:expr) => {
        match bound_session(&$app, &$h, &$f) {
            Ok(s) => s,
            Err(r) => return *r,
        }
    };
}

fn rev(f: &Fields) -> u64 {
    num(f, "state_rev").unwrap_or(0)
}

pub async fn post_session(State(app): State<Arc<App>>, h: HeaderMap, Form(f): F) -> Response {
    guard!(app, h, f);
    let mode = if f.get("mode").map(String::as_str) == Some("lunch") {
        Mode::Lunch
    } else {
        Mode::Negotiation
    };
    app.create_session(mode);
    back()
}

pub async fn post_close(State(app): State<Arc<App>>, h: HeaderMap, Form(f): F) -> Response {
    let _sh = session!(app, h, f);
    app.close_active();
    back()
}

pub async fn post_goals(State(app): State<Arc<App>>, h: HeaderMap, Form(f): F) -> Response {
    let sh = session!(app, h, f);
    let size = f
        .get("size")
        .and_then(|v| EnumValue::parse_label(v))
        .filter(|v| v.is_size());
    let firm = f
        .get("firmness")
        .and_then(|v| EnumValue::parse_label(v))
        .filter(|v| v.is_firmness());
    let budget: Option<u64> = num(&f, "budget").filter(|b| *b > 0 && *b <= 1_000_000);
    let nights: Option<u32> = num(&f, "nights").filter(|n| *n <= 3650);
    let (Some(size), Some(firm), Some(budget), Some(nights)) = (size, firm, budget, nights) else {
        return error_page(StatusCode::UNPROCESSABLE_ENTITY, "invalid goals");
    };
    let goals = NegGoals {
        sizes: vec![size],
        firmness: vec![firm],
        budget_cents: budget * 100,
        min_return_nights: nights,
    };
    let gv = num(&f, "goals_version").unwrap_or(0);
    mutate(&app, &sh, |s| s.set_goals(gv, goals))
}

/// Typed turn: same admission path as the bridge, labelled "typed".
pub async fn post_turn(State(app): State<Arc<App>>, h: HeaderMap, Form(f): F) -> Response {
    let sh = session!(app, h, f);
    let text = f.get("text").cloned().unwrap_or_default();
    let nonce = f.get("nonce").cloned().unwrap_or_default();
    let req = AcceptReq {
        mode: sh.mode,
        source_id: "ui".into(),
        client_turn_id: format!("ui-{}", copilot_core::text::bounded(&nonce, 64).0),
        source_seq: None,
        text,
        transcript_source: "typed".into(),
        speaker: None,
        input_source: if h.contains_key("x-copilot-simulated") {
            "simulated".into()
        } else {
            "typed".into()
        },
        now_ms: app.now_ms(),
    };
    let r = {
        let mut s = sh.state.lock().expect("session");
        let r = s.accept_turn(req);
        sh.publish(&s);
        r
    };
    match r {
        Ok(a) => {
            let seq = match a {
                copilot_core::session::Accepted::New(s)
                | copilot_core::session::Accepted::Duplicate(s) => s,
            };
            crate::api::wait_turn(&sh, seq, copilot_core::limits::TURN_RESPONSE_WAIT_MS).await;
            back()
        }
        Err(e) => error_page(StatusCode::CONFLICT, &format!("{}: {}", e.code, e.message)),
    }
}

fn parse_items(f: &Fields) -> Vec<(u64, u64)> {
    f.get("items")
        .map(|v| {
            v.split(',')
                .filter_map(|p| p.split_once(':'))
                .filter_map(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?)))
                .take(64)
                .collect()
        })
        .unwrap_or_default()
}

pub async fn post_confirm(State(app): State<Arc<App>>, h: HeaderMap, Form(f): F) -> Response {
    let sh = session!(app, h, f);
    let items = parse_items(&f);
    let offer = f.get("offer_id").cloned().unwrap_or_default();
    let now = app.now_ms();
    mutate(&app, &sh, |s| {
        s.confirm_facts(rev(&f), &offer, &items, now).map(|_| ())
    })
}

pub async fn post_correct(State(app): State<Arc<App>>, h: HeaderMap, Form(f): F) -> Response {
    let sh = session!(app, h, f);
    let v = f
        .get("value")
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let c = match f.get("field").map(String::as_str) {
        Some("total") => parse_amount_text(&v).map(Correction::TotalInclDelivery),
        Some("size") => EnumValue::parse_label(&v)
            .filter(|e| e.is_size())
            .map(Correction::Size),
        Some("firmness") => EnumValue::parse_label(&v)
            .filter(|e| e.is_firmness())
            .map(Correction::Firmness),
        Some("return") => v
            .parse::<u32>()
            .ok()
            .filter(|n| *n <= 3650)
            .map(Correction::ReturnNights),
        _ => None,
    };
    let Some(c) = c else {
        return error_page(
            StatusCode::UNPROCESSABLE_ENTITY,
            "value does not fit the field",
        );
    };
    let offer = f.get("offer_id").cloned().unwrap_or_default();
    let now = app.now_ms();
    mutate(&app, &sh, |s| s.correct_value(rev(&f), &offer, c, now))
}

pub async fn post_unresolved_dismiss(
    State(app): State<Arc<App>>,
    h: HeaderMap,
    Form(f): F,
) -> Response {
    let sh = session!(app, h, f);
    let offer = f.get("offer_id").cloned().unwrap_or_default();
    mutate(&app, &sh, |s| {
        s.dismiss_unresolved(
            rev(&f),
            &offer,
            num(&f, "id").unwrap_or(0),
            num(&f, "version").unwrap_or(0),
        )
    })
}

pub async fn post_resolve_condition(
    State(app): State<Arc<App>>,
    h: HeaderMap,
    Form(f): F,
) -> Response {
    let sh = session!(app, h, f);
    let offer = f.get("offer_id").cloned().unwrap_or_default();
    mutate(&app, &sh, |s| {
        s.resolve_condition(
            rev(&f),
            &offer,
            num(&f, "cond_id").unwrap_or(0),
            num(&f, "version").unwrap_or(0),
        )
    })
}

pub async fn post_discard(State(app): State<Arc<App>>, h: HeaderMap, Form(f): F) -> Response {
    let sh = session!(app, h, f);
    mutate(&app, &sh, |s| {
        s.discard_turn(rev(&f), num(&f, "seq").unwrap_or(0))
    })
}

pub async fn post_ambiguous(State(app): State<Arc<App>>, h: HeaderMap, Form(f): F) -> Response {
    let sh = session!(app, h, f);
    let how = match f.get("how").map(String::as_str) {
        Some("new_offer") => AmbiguousResolution::NewOffer,
        Some("amendment") => AmbiguousResolution::Amendment,
        Some("ignore") => AmbiguousResolution::Ignore,
        _ => return error_page(StatusCode::UNPROCESSABLE_ENTITY, "choose how to resolve"),
    };
    mutate(&app, &sh, |s| {
        s.resolve_ambiguous(rev(&f), num(&f, "seq").unwrap_or(0), how)
    })
}

pub async fn post_eval_retry(State(app): State<Arc<App>>, h: HeaderMap, Form(f): F) -> Response {
    let sh = session!(app, h, f);
    mutate(&app, &sh, |s| s.retry_evaluation(rev(&f)))
}

pub async fn post_retry(State(app): State<Arc<App>>, h: HeaderMap, Form(f): F) -> Response {
    let sh = session!(app, h, f);
    let seq = num(&f, "seq").unwrap_or(0);
    let r = {
        let mut s = sh.state.lock().expect("session");
        let r = if rev(&f) != s.state_rev {
            Err(ApiError::new(
                "stale_form",
                "state changed since this form was loaded",
                false,
            ))
        } else {
            s.retry_turn(seq, num(&f, "attempt").unwrap_or(0))
                .map(|_| ())
        };
        sh.publish(&s);
        r
    };
    match r {
        Ok(()) => {
            crate::api::wait_turn(&sh, seq, copilot_core::limits::TURN_RESPONSE_WAIT_MS).await;
            back()
        }
        Err(e) if e.code == "stale_form" => stale_page(&app, &e.message),
        Err(e) => error_page(StatusCode::UNPROCESSABLE_ENTITY, &e.message),
    }
}

// ---- lunch ----

fn lunch_op(
    app: &App,
    sh: &SessionHandle,
    f: &Fields,
    op: impl FnOnce(&mut lunch::LunchState) -> Result<(), String>,
) -> Response {
    mutate(app, sh, |s| {
        if rev(f) != s.state_rev {
            return Err(ApiError::new(
                "stale_form",
                "options changed since this form was loaded",
                false,
            ));
        }
        let Some(l) = s.lunch.as_mut() else {
            return Err(ApiError::new("not_lunch", "not a lunch session", false));
        };
        op(l).map_err(|e| ApiError::new("invalid", e, false))?;
        s.lunch_setup_changed();
        Ok(())
    })
}

pub async fn post_lunch_fixture(State(app): State<Arc<App>>, h: HeaderMap, Form(f): F) -> Response {
    let sh = session!(app, h, f);
    lunch_op(&app, &sh, &f, |l| {
        l.load_fixture();
        l.setup_confirmed = true;
        Ok(())
    })
}

fn parse_hhmm(v: &str) -> Option<u16> {
    let (h, m) = v.trim().split_once(':')?;
    let (h, m): (u16, u16) = (h.parse().ok()?, m.parse().ok()?);
    (h <= 24 && m < 60 && h * 60 + m <= 1440).then_some(h * 60 + m)
}

pub async fn post_lunch_setup(State(app): State<Arc<App>>, h: HeaderMap, Form(f): F) -> Response {
    let sh = session!(app, h, f);
    let names: Vec<String> = (0..3)
        .map(|i| {
            f.get(&format!("name{i}"))
                .map(|s| s.trim().to_string())
                .unwrap_or_default()
        })
        .collect();
    let (Some(ws), Some(we)) = (
        f.get("win_start").and_then(|v| parse_hhmm(v)),
        f.get("win_end").and_then(|v| parse_hhmm(v)),
    ) else {
        return error_page(StatusCode::UNPROCESSABLE_ENTITY, "invalid window");
    };
    if names.iter().any(|n| n.is_empty() || n.len() > 80) || ws >= we {
        return error_page(
            StatusCode::UNPROCESSABLE_ENTITY,
            "lunch requires exactly three named options and a valid window",
        );
    }
    lunch_op(&app, &sh, &f, |l| {
        for (i, n) in names.iter().enumerate() {
            l.rename_option(i, n);
        }
        l.window = (ws, we);
        l.setup_confirmed = true;
        l.setup_version += 1;
        Ok(())
    })
}

pub async fn post_lunch_fact(State(app): State<Arc<App>>, h: HeaderMap, Form(f): F) -> Response {
    let sh = session!(app, h, f);
    let idx = match f.get("opt").map(String::as_str) {
        Some("A") => 0,
        Some("B") => 1,
        Some("C") => 2,
        _ => return error_page(StatusCode::UNPROCESSABLE_ENTITY, "option"),
    };
    let v = f.get("value").cloned().unwrap_or_default();
    let field = match f.get("kind").map(String::as_str) {
        Some("peanut_free") => LunchField::Allergen {
            allergen: Allergen::Peanut,
            status: AllergenStatus::FreeOf,
        },
        Some("peanut_contains") => LunchField::Allergen {
            allergen: Allergen::Peanut,
            status: AllergenStatus::Contains,
        },
        Some("distance") => match Distance::parse(&v, DistUnit::Mi) {
            Some(d) => LunchField::Distance { distance: d },
            None => {
                return error_page(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "distance must match the supported grammar",
                )
            }
        },
        Some("hours") => match v
            .split_once('-')
            .and_then(|(a, b)| Some((parse_hhmm(a)?, parse_hhmm(b)?)))
        {
            Some((a, b)) if a < b => LunchField::Hours {
                intervals: vec![(a, b)],
            },
            _ => {
                return error_page(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "hours must be HH:MM-HH:MM",
                )
            }
        },
        Some("veg_yes") => LunchField::Diet {
            diet: Diet::Vegetarian,
            has: true,
        },
        Some("veg_no") => LunchField::Diet {
            diet: Diet::Vegetarian,
            has: false,
        },
        _ => return error_page(StatusCode::UNPROCESSABLE_ENTITY, "kind"),
    };
    lunch_op(&app, &sh, &f, |l| {
        l.set_user_fact(idx, field, "user entry");
        Ok(())
    })
}

pub async fn post_lunch_unresolved(
    State(app): State<Arc<App>>,
    h: HeaderMap,
    Form(f): F,
) -> Response {
    let sh = session!(app, h, f);
    let (id, ver) = (num(&f, "id").unwrap_or(0), num(&f, "version").unwrap_or(0));
    lunch_op(&app, &sh, &f, |l| {
        if l.dismiss_unresolved(id, ver) {
            Ok(())
        } else {
            Err("interpretation changed".into())
        }
    })
}

pub async fn post_lunch_confirm(State(app): State<Arc<App>>, h: HeaderMap, Form(f): F) -> Response {
    let sh = session!(app, h, f);
    let items: Vec<(usize, u64, u64)> = f
        .get("items")
        .map(|v| {
            v.split(',')
                .filter_map(|p| {
                    let mut it = p.split(':');
                    let letter = it.next()?;
                    let l = lunch::LETTERS.iter().position(|x| *x == letter)?;
                    Some((l, it.next()?.parse().ok()?, it.next()?.parse().ok()?))
                })
                .take(64)
                .collect()
        })
        .unwrap_or_default();
    lunch_op(&app, &sh, &f, |l| {
        let ok = items.iter().all(|(o, id, v)| {
            l.options[*o]
                .facts
                .iter()
                .any(|x| x.id == *id && x.version == *v)
        });
        if !ok {
            return Err("a fact changed since this form was loaded".into());
        }
        for (o, id, _) in &items {
            if let Some(x) = l.options[*o].facts.iter_mut().find(|x| x.id == *id) {
                x.confirmed = true;
                x.version += 1;
            }
            l.options[*o].version += 1;
        }
        Ok(())
    })
}

pub async fn post_pref_add(State(app): State<Arc<App>>, h: HeaderMap, Form(f): F) -> Response {
    guard!(app, h, f);
    let v = f.get("value").cloned().unwrap_or_default();
    let (kind, strength) = match f.get("kind").map(String::as_str) {
        Some("allergen_peanut") => (
            PrefKind::Allergen {
                allergen: Allergen::Peanut,
            },
            Strength::Hard,
        ),
        Some("max_walk") => match Distance::parse(&v, DistUnit::Mi) {
            Some(d) => (PrefKind::MaxWalk { distance: d }, Strength::Hard),
            None => {
                return error_page(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "distance must match the supported grammar",
                )
            }
        },
        Some("prefer_veg") => (
            PrefKind::Diet {
                diet: Diet::Vegetarian,
                prefer: true,
            },
            Strength::Soft,
        ),
        Some("avoid_veg") => (
            PrefKind::Diet {
                diet: Diet::Vegetarian,
                prefer: false,
            },
            Strength::Soft,
        ),
        _ => return error_page(StatusCode::UNPROCESSABLE_ENTITY, "kind"),
    };
    let r = app
        .memory
        .apply(
            num(&f, "memory_publication").unwrap_or(u64::MAX),
            Change::Add {
                kind,
                strength,
                note: String::new(),
            },
        )
        .await;
    save_result(&app, r)
}

pub async fn post_pref_delete(State(app): State<Arc<App>>, h: HeaderMap, Form(f): F) -> Response {
    guard!(app, h, f);
    let r = app
        .memory
        .apply(
            num(&f, "memory_publication").unwrap_or(u64::MAX),
            Change::Delete {
                id: num(&f, "id").unwrap_or(0),
                version: num(&f, "version").unwrap_or(0),
            },
        )
        .await;
    save_result(&app, r)
}

fn save_result(app: &App, r: SaveResult) -> Response {
    match r {
        SaveResult::Saved(_) => back(),
        SaveResult::Stale => stale_page(app, "preferences changed since this form was loaded"),
        SaveResult::NotSaved(why) => error_page(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Not saved: {why}"),
        ),
        SaveResult::Uncertain(why) => error_page(
            StatusCode::SERVICE_UNAVAILABLE,
            &format!("Save outcome unknown: {why}. Load latest to check."),
        ),
    }
}

pub async fn post_memory_recheck(
    State(app): State<Arc<App>>,
    h: HeaderMap,
    Form(f): F,
) -> Response {
    guard!(app, h, f);
    let r = app.memory.recheck().await;
    save_result(&app, r)
}

pub async fn post_memory_new_profile(
    State(app): State<Arc<App>>,
    h: HeaderMap,
    Form(f): F,
) -> Response {
    guard!(app, h, f);
    if f.get("confirm_text").map(|s| s.trim()) != Some("START NEW PROFILE") {
        return error_page(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Type START NEW PROFILE exactly. Nothing changed.",
        );
    }
    let r = app
        .memory
        .new_profile(f.get("fingerprint").cloned().unwrap_or_default())
        .await;
    save_result(&app, r)
}
