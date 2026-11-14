'use client';
import { useState } from 'react';
import Conversation from '@/components/Conversation';
import DecisionCard from '@/components/DecisionCard';
import GoalsForm from '@/components/GoalsForm';
import { setSession } from '@/lib/api';
import { selectedTurn, visibleTurns, type Mode, type Session } from '@/lib/store';
import { useBridge } from '@/lib/useBridge';

const MIC = { live: '● Mic live', 'cue-hold': 'Mic paused for cue', off: 'Mic off' } as const;

export default function Live() {
  const { state, dispatch, link } = useBridge();
  const [offline, setOffline] = useState(false);
  const { session, status } = state;

  const save = (next: Session, post: boolean) => {
    dispatch({ type: 'session', session: next });
    if (post) setSession(next).then(() => setOffline(false), () => setOffline(true));
  };

  const sel = selectedTurn(state);
  const turn = sel == null ? undefined : state.turns[sel];
  const d = turn?.decisionId ? state.decisions[turn.decisionId] : undefined;
  const goalsOk = session.goals.length > 0 && session.goals.every((g) => g.confirmed);
  const empty = visibleTurns(state).length === 0;
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
        </header>
        {(status?.mic === 'live' || micUnknown) && (
          <div className="banner" role="status">
            {micUnknown
              ? 'Bridge connected, mic state not reported: assume this conversation is being recorded and sent to Deepgram.'
              : 'Recording — audio goes to Deepgram.'}
          </div>
        )}
        <div className="thread">
          {empty ? (
            <div className="empty">
              <h1>What are we deciding?</h1>
              <p className="muted">
                Set your goals below, start the bridge with <code>cargo run -- --band &lt;band number&gt;</code> and
                start talking. Each speaker's words stream in here as they are spoken.
              </p>
            </div>
          ) : (
            <Conversation state={state} onPick={(turn_id) => dispatch({ type: 'pin', turn_id })} />
          )}
        </div>
        <div className="dock">
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
      </aside>
    </div>
  );
}
