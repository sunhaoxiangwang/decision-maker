'use client';
import Link from 'next/link';
import { useState } from 'react';
import ContextConfirm from '@/components/ContextConfirm';
import Conversation from '@/components/Conversation';
import DecisionCard from '@/components/DecisionCard';
import GoalsForm from '@/components/GoalsForm';
import { setSession } from '@/lib/api';
import { decisionRequest } from '@/lib/memory';
import { useNames } from '@/lib/names';
import { selectedTurn, visibleTurns, type Mode, type Session } from '@/lib/store';
import { useBridge } from '@/lib/useBridge';

const MIC = { live: '● Mic live', 'cue-hold': 'Mic paused for cue', off: 'Mic off' } as const;

export default function Live() {
  const { state, dispatch, link } = useBridge();
  const [offline, setOffline] = useState(false);
  const { session, status } = state;
  const { name } = useNames();

  const save = (next: Session, post: boolean) => {
    dispatch({ type: 'session', session: next });
    if (post) setSession(next).then(() => setOffline(false), () => setOffline(true));
  };

  const sel = selectedTurn(state);
  const turn = sel == null ? undefined : state.turns[sel];
  const d = turn?.decisionId ? state.decisions[turn.decisionId] : undefined;
  const goalsOk = session.goals.length > 0 && session.goals.every((g) => g.confirmed);
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
          <select aria-label="Mode" value={session.mode} onChange={(e) => save({ ...session, mode: e.target.value as Mode }, true)}>
            <option value="negotiation">Negotiation</option>
            <option value="lunch">Lunch</option>
          </select>
          <span className="chip">{goalsOk ? 'Goals ✓' : 'Goals not confirmed'}</span>
          <span className={`chip${status?.mic === 'live' ? ' rec' : ''}`}>{status ? MIC[status.mic] : 'Mic unknown'}</span>
          {state.decisionsOn !== null && (
            <span className={`chip${state.decisionsOn ? ' on' : ''}`} title="Triple-tap the band to switch">
              Decisions {state.decisionsOn ? 'on' : 'off'}
            </span>
          )}
          {!empty && state.decisionsOn !== false && (
            <button className="ghost" title="Keep this conversation in Memory › Past conversations and start a new one"
              onClick={() => dispatch({ type: 'new-conversation' })}>New conversation</button>
          )}
        </header>
        {(status?.mic === 'live' || micUnknown) && (
          <div className="banner" role="status">
            {micUnknown
              ? 'Bridge connected, mic state not reported: assume this conversation is being recorded and sent to Deepgram.'
              : 'Recording — audio goes to Deepgram.'}
          </div>
        )}
        <div className="thread">
          {state.decisionsOn === false ? (
            <div className="empty">
              <h1>Decisions mode is off</h1>
              <p className="muted">
                Triple-tap the band to start. It blinks green five times, then listens: a new conversation opens here
                and each speaker&apos;s words stream in. Triple-tap again to stop (red five times); nothing is heard or saved while it&apos;s off.
                Past conversations are in <Link href="/memory">Memory</Link>.
              </p>
            </div>
          ) : empty ? (
            <div className="empty">
              <h1>What are we deciding?</h1>
              <p className="muted">
                {state.decisionsOn
                  ? "Decisions mode is on: start talking. Each speaker's words stream in here as they are spoken."
                  : <>Start the bridge with <code>cargo run -- --band &lt;band number&gt;</code>, then triple-tap the band to start decisions mode.</>}
              </p>
            </div>
          ) : (
            <Conversation state={state} onPick={(turn_id) => dispatch({ type: 'pin', turn_id })} />
          )}
        </div>
        <div className="dock">
          <ContextConfirm state={state} dispatch={dispatch} />
          <GoalsForm session={session} save={save} />
          <p className="fine">
            {offline
              ? 'Orchestrator not reachable: mode and goals are saved in this browser but were not sent.'
              : 'Listening only counts once your goals are confirmed.'}
          </p>
        </div>
      </section>
      <aside className="pane">
        <header>
          <h2>Decision{sel != null && ` · turn ${sel}`}</h2>
          {state.pinned != null && <button className="ghost" onClick={() => dispatch({ type: 'pin', turn_id: null })}>Follow latest</button>}
        </header>
        {sel == null
          ? <p className="muted">The latest decision shows here: the cue, why, and the facts and memory it used. Click any turn to pin its decision.</p>
          : <DecisionCard turn={turn} d={d} options={session.lunch_options} />}
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
