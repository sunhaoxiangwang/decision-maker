'use client';
import Link from 'next/link';
import MemoryCapture from '@/components/MemoryCapture';
import Conversation from '@/components/Conversation';
import { CueScale } from '@/components/CueBadge';
import { decisionRequest } from '@/lib/memory';
import { useNames } from '@/lib/names';
import { selectedTurn, visibleTurns } from '@/lib/store';
import { useBridge } from '@/lib/useBridge';

const MIC = { live: '● Mic live', 'cue-hold': 'Mic paused for cue', off: 'Mic off' } as const;

export default function Live() {
  const { state, dispatch, link } = useBridge();
  const { status } = state;
  const { name } = useNames();

  const sel = selectedTurn(state);
  const turn = sel == null ? undefined : state.turns[sel];
  const d = turn?.decisionId ? state.decisions[turn.decisionId] : undefined;
  const empty = visibleTurns(state).length === 0;
  // What the decision API would be sent for the selected (or latest) finished turn: the
  // memory shown here is exactly what retrieval picks for it.
  const conv = state.current ? state.conversations[state.current] : undefined;
  const at = sel ?? conv?.turns.at(-1)?.turn_id;
  const request = conv && at != null ? decisionRequest(conv, at, Object.values(state.memory), name) : null;
  // Safety rule: always disclose a live mic. If nobody has reported the mic state but the
  // bridge is up, assume it is live.
  const micUnknown = !status && link === 'open';

  return (
    <div className="split">
      <section className="chat">
        <header className="topbar">
          <h1 className="title">Live</h1>
          <span className={`chip${status?.mic === 'live' ? ' rec' : ''}`}>{status ? MIC[status.mic] : 'Mic unknown'}</span>
          {state.decisionsOn !== null && (
            <span className={`chip${state.decisionsOn ? ' on' : ''}`} title="Triple-tap the band to switch">
              Decisions {state.decisionsOn ? 'on' : 'off'}
            </span>
          )}
          {state.context && <span className="chip rec">Recording memory</span>}
          {!empty && state.decisionsOn !== false && (
            <button className="ghost" title="Keep this conversation in Memory › Past conversations and start a new one"
              onClick={() => dispatch({ type: 'new-conversation' })}>New conversation</button>
          )}
        </header>
        {micUnknown && (
          <div className="banner" role="status">
            Bridge connected, mic state not reported: assume this conversation is being recorded and sent to Deepgram.
          </div>
        )}
        <div className="thread">
          {state.decisionsOn === false ? (
            <div className="empty">
              <h1>Decisions mode is off</h1>
              <p className="muted">
                Triple-tap the band to start. It blinks green five times, then listens: a new conversation opens here
                and each speaker&apos;s words stream in. Triple-tap again to stop (red five times).
                Wave to record memory; wave again to stop. Memory capture also works with decisions off.
                Past conversations are in <Link href="/memory">Memory</Link>.
              </p>
            </div>
          ) : empty ? (
            <div className="empty">
              <h1>Live transcript</h1>
              <p className="muted">
                {state.decisionsOn
                  ? 'Listening. Your words appear on the right, everyone else on the left, and the copilot answers in its own card.'
                  : <>Start the bridge with <code>cargo run -- --band &lt;band number&gt;</code>, then triple-tap the band to start decisions mode.</>}
              </p>
            </div>
          ) : (
            <Conversation state={state} onPick={(turn_id) => dispatch({ type: 'pin', turn_id })} />
          )}
        </div>
        <div className="dock">
          <MemoryCapture state={state} />
          <p className="fine">
            Goals are free text in the copilot (http://127.0.0.1:8787/); a wave adds what you say to them.
          </p>
        </div>
      </section>
      <aside className="pane">
        <header>
          <h2>Decision{sel != null && ` · turn ${sel}`}</h2>
          {state.pinned != null && <button className="ghost" onClick={() => dispatch({ type: 'pin', turn_id: null })}>Follow latest</button>}
        </header>
        {/* fixed-size window: the three lights never move; only the current call lights up */}
        <div className="decision-window">
          <CueScale cue={turn?.cue ?? d?.cue ?? 'Off'} />
          <p className="decision-reason">{d?.reason ?? (sel == null ? 'Waiting for the first finished turn.' : 'No decision for this turn.')}</p>
        </div>
        <section>
          <h3>Memory for {request ? `turn ${request.turn.turn_id} · ${request.turn.speaker}` : 'the next turn'}</h3>
          {!request || request.memory.length === 0 ? (
            <p className="muted">
              {Object.keys(state.memory).length === 0
                ? <>No memory yet. Add context or remember a past conversation in <Link href="/memory">Memory</Link>.</>
                : 'No confirmed memory matches this turn.'}
            </p>
          ) : (
            <ul>
              {request.memory.map((m) => (
                <li key={m.id}>
                  <Link href={`/memory#m-${encodeURIComponent(m.id)}`}>{m.text}</Link>
                  <small> · {m.kind} · {m.subject}</small>
                </li>
              ))}
            </ul>
          )}
        </section>
      </aside>
    </div>
  );
}
