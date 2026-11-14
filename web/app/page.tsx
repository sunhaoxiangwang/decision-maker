'use client';
import { useState } from 'react';
import Conversation from '@/components/Conversation';
import DecisionCard from '@/components/DecisionCard';
import GoalsForm from '@/components/GoalsForm';
import { api } from '@/lib/api';
import { selectedTurn, type Mode, type Session } from '@/lib/store';
import { useBridge } from '@/lib/useBridge';

const MIC = { live: '● Mic live', 'cue-hold': 'Mic paused for cue', off: 'Mic off' } as const;

export default function Live() {
  const { state, dispatch, link } = useBridge();
  const [offline, setOffline] = useState(false);
  const { session, status, mock } = state;

  const save = (next: Session, post: boolean) => {
    dispatch({ type: 'session', session: next });
    if (post) api.setSession(next).then(() => setOffline(false), () => setOffline(true));
  };

  const sel = selectedTurn(state);
  const turn = sel == null ? undefined : state.turns[sel];
  const d = turn?.decisionId ? state.decisions[turn.decisionId] : undefined;
  const goalsOk = session.goals.length > 0 && session.goals.every((g) => g.confirmed);
  const empty = Object.keys(state.turns).length === 0;
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
          {mock && <span className="chip mock">Mock data</span>}
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
                Set your goals below, then start the bridge with <code>cargo run -- --typed --cue</code>.
                Turns appear here as they are spoken. No bridge yet? Load a mock demo from the sidebar.
              </p>
            </div>
          ) : (
            <Conversation state={state} onPick={(turn_id) => dispatch({ type: 'pin', turn_id })} />
          )}
        </div>
        <div className="dock">
          <GoalsForm session={session} save={save} />
          <p className="fine">
            {offline && !mock
              ? 'Orchestrator not reachable: mode and goals are kept in this tab only.'
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
