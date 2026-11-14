# `vexo-copilot/1` — bridge contract (proposal for Cy)

Status: **proposed by Leo's app; not yet agreed by Cy.** Agreement is recorded only when Cy confirms it.

Leo's app (this repository) decides and publishes cues. Cy's bridge captures audio, transcribes it, connects to the band and applies LEDs. The app never talks to the band.

## Transport

- HTTP/1.1 JSON on **loopback only**: `http://127.0.0.1:8787`. The bridge must run on the same laptop as the app.
- Every bridge request: `Authorization: Bearer $COPILOT_BRIDGE_TOKEN` (Leo generates it and shares it with Cy outside chat and the repo). POST bodies: `Content-Type: application/json`, at most 16 KiB.
- Sessions are created and closed only in the companion UI. One active session at a time.

## Endpoints

| Endpoint | Purpose |
|---|---|
| `GET /v1/sessions/active` | `session_info`: `session_id`, `mode`, `rev`, `instance_id`, `latest_generation`, `source_latest[]` |
| `POST /v1/sessions/{sid}/turns` | Submit a transcript turn; returns `turn_result` within 15 s (`PENDING` if still running) |
| `GET /v1/sessions/{sid}/turns/{client_turn_id}` | Look up a submission after a timeout (`turn_result` with `snapshot:null`, or 404 `not_accepted`) |
| `POST /v1/sessions/{sid}/turns/{client_turn_id}/cancel` | `cancel_ack` (never actionable) |
| `POST /v1/sessions/{sid}/turns/{client_turn_id}/retry` body `{"attempt": n}` | `retry_ack` (never actionable) |
| `GET /v1/sessions/{sid}/cue?after_rev=N&wait_ms=W` | `cue_snapshot`, waiting up to `min(W, 20000)` ms for `rev > N`; at most 8 waiting polls |
| `POST /v1/sessions/{sid}/cue-reports` | Optional latency report (≤ 1 KiB, strictly validated: protocol, a published `rev`, `applied` = `led_set`/`led_off`, known duration keys ≤ 600000 ms); recorded as *bridge-reported*, not hardware validation |
| `GET /v1/diagnostics/occupancy` | Retention and admission counters (used by the stress check) |

## Turn request

```json
{"protocol":"vexo-copilot/1","session_id":"<sid>","mode":"negotiation","source_id":"cy-bridge",
 "client_turn_id":"<bridge_boot>-<epoch>","source_seq":7,
 "transcript":{"text":"Best I can do is $1,150 delivered.","transcript_source":"deepgram","speaker":"counterpart"}}
```

- `source_seq` strictly increases per bridge. `client_turn_id` is unique per submission; resending the identical body is idempotent.
- `speaker` is informational only. Transcript text never confirms anything.

## Message shapes

Every message has the header: `protocol, request_id, instance_id, kind, session_id, mode, rev, state_rev, generation, error`.

- `cue_snapshot` (the **only** kind a bridge may apply): `current, phase, outcome, selected_option, cue, actionable, reason, missing[], conflicts[], unconfirmed[], conditions[], barriers[], truncated, provenance{input_source, transcript_source, providers, pipeline:"rust", decision_source}, timings_ms{accept_to_publish, extract, evaluate, decide}`.
- Timings (`timings_ms`, server monotonic clock): `accept_to_publish`, `extract`, `evaluate`, `decide`, plus generation-level `e2e` (first acceptance → latest publication, including confirmation waits), `human_wait` and `processing`.
- `turn_result`: `turn{source_id, client_turn_id, source_seq, server_seq, attempt, attempts_total}, turn_outcome (PENDING | APPLIED | APPLIED_SUPERSEDED | FAILED_BEFORE_APPLY | FAILED_AFTER_APPLY | CANCELLED | DISCARDED), turn_effect, snapshot` — `snapshot` is non-null only for `APPLIED` when `generation == server_seq`. Duplicates return the turn's latest attempt with `"duplicate": true`.
- `cancel_ack`, `retry_ack`, `report_ack`, `session_info`, `error`: never actionable; `cue:null`.

## Cues (proposals until Cy verifies them on the band)

| Outcome | LED |
|---|---|
| PROCEED | `#00FF00` solid |
| PAUSE | `#FF0000` blink 400 ms |
| ASK | `#FFB000` blink 1000 ms |
| SELECT A / B / C | `#00FF00` / `#8000FF` / `#FFFFFF` solid |
| processing, invalidated, closed, NO_ELIGIBLE_OPTION, NONE | `clear` (LED off) |
| UNAVAILABLE | `#00FFFF` blink 150 ms |

The band has no vibration motor, speaker or screen; cues are LED only.

## Bridge requirements (Cy implements; reference model in `crates/copilot-core/src/bridge_model.rs`)

1. Call `led_off()` **before** turning the microphone on, so the setting restored after the mic stops is "off".
2. Stop **both** audio and mic-level streams (`configure(audio=False, mic_level=False)`) before any custom LED.
3. Keep a local capture epoch. `client_turn_id = "<boot>-<epoch>"`. While capturing, or while awaiting the response for the current epoch, apply no cues.
4. A turn response (or lookup) only clears "awaiting" for the matching `client_turn_id` and the same instance/session/mode. `min_generation = max(min_generation, server_seq)` — never decreases.
5. Apply a cue only if: `kind == "cue_snapshot"` (or an embedded snapshot), `actionable`, instance + session + mode all match, not capturing, not awaiting, `generation >= min_generation`, and `rev > last_rev`. Then `last_rev = rev`.
6. On a new instance, session or mode from `GET /v1/sessions/active`: `led_off()`, reset `last_rev` and `min_generation` to 0, clear awaiting.
7. Submission timeout: look up the `client_turn_id`. Lookup results (including 404s and errors) for any other submission, session or instance are ignored and never change epoch state. Found → adopt its `server_seq`. 404 → resend the identical body up to 2 times, then fail closed (`led_off()`, keep blocking cues until a newer epoch's turn is accepted).
8. Disclose recording and Deepgram transcription to participants before capture.

## Latency evidence

Cy reports durations measured on the bridge's own monotonic clock (never absolute timestamps across laptops):

```json
{"protocol":"vexo-copilot/1","rev":16,"applied":"led_set",
 "bridge_durations_ms":{"mic_off_to_transcript_final":640,"submit_to_turn_response":1260,"submit_to_final_cue_applied_ms":5400}}
```

`submit_to_final_cue_applied_ms` runs from sending the turn to applying the last actionable cue for that generation, including any confirmation wait. The server reports its own processing and human-wait intervals separately.

## Errors

| HTTP | Codes |
|---|---|
| 400 | `bad_request`, `unsupported_protocol` (framework body/JSON/query rejections also use this JSON envelope) |
| 401 / 403 | `unauthorized`, `source_mismatch` |
| 404 | `session_not_found`, `not_accepted`, `no_active_session` |
| 409 | `session_closed`, `session_mismatch`, `mode_mismatch`, `stale_turn`, `turn_id_reused` |
| 422 | `field_too_long` (client_turn_id ≤ 128 B, source_id ≤ 64 B, transcript_source/speaker ≤ 32 B) |
| 429 | `too_many_unresolved_turns` (16 failed + unclear turns must be retried, discarded or resolved first) |
| 412 | `goals_unconfirmed` |
| 413 / 415 | body over 16 KiB / wrong content type |
| 422 | `empty_transcript`, `transcript_too_long` |
| 429 | `turn_queue_full`, `too_many_pollers` |
| 503 | `server_busy` (64 requests in flight, including waiting polls), `request_timeout` (20 s; polls 25 s), `bridge_disabled` |
| 200 + `UNAVAILABLE` | `missing_credentials`, `extraction_*`, `decision_*`, `provider_invalid_output`, `job_deadline`, `queue_timeout`, `capacity_unavailable`, `unresolved_failed_turn`, `memory_recovery_required` |
