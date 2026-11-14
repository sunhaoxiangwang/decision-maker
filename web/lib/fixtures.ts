// MOCK data for build step 2: the four negotiation cases and two lunch cases from the
// demo check in docs/frontend-plan.md. The UI labels anything loaded from here as mock.
import type { BridgeEvent, Cue, DecisionEvent, MemoryItem, Session } from './store';

export type Demo = { session: Session; events: BridgeEvent[] };

const MIN = 60_000;

export function negotiationDemo(now = Date.now()): Demo {
  const t = (n: number) => now - 2 * MIN + n * 9000; // one turn every 9 s
  const budget: MemoryItem = { id: 'm1', key: 'budget_total', value: '≤ $1,200 incl. delivery', status: 'confirmed', source: 'user', saved_ms: now - 120 * MIN, last_used_ms: t(5), used_by: 3 };
  const nights: MemoryItem = { id: 'm2', key: 'return_nights', value: '≥ 100 nights', status: 'confirmed', source: 'goal · this session', saved_ms: now - 5 * MIN, last_used_ms: t(5), used_by: 3 };
  const firm: MemoryItem = { id: 'm3', key: 'preferred_firmness', value: 'medium', status: 'proposed', source: 'summary · session 3', saved_ms: now - 26 * 60 * MIN, used_by: 0 };
  const ref = (m: MemoryItem, at: number) => ({ id: m.id, key: m.key, value: m.value, source: m.source, retrieved_ms: at });
  const say = (turn_id: number, speaker: number, text: string, final = true): BridgeEvent =>
    ({ type: 'transcript', turn_id, speaker, text, final, time_ms: t(turn_id) });
  const decide = (turn_id: number, cue: Cue, shown: boolean, d: Pick<DecisionEvent, 'reason' | 'facts' | 'missing' | 'conflicts' | 'memory_used'>): BridgeEvent[] => [
    { type: 'decision', id: `d${turn_id}`, turn_id, mode: 'negotiation', cue, ...d, latency_ms: { transcript_to_decision: 180 + turn_id * 35, decision_to_led: shown ? 40 : undefined }, time_ms: t(turn_id) + 300 },
    { type: 'cue', turn_id, cue, shown, time_ms: t(turn_id) + 340 },
  ];
  return {
    session: {
      mode: 'negotiation',
      goals: [
        { key: 'budget_total', value: '≤ 1200', confirmed: true },
        { key: 'return_nights', value: '≥ 100', confirmed: true },
      ],
    },
    events: [
      { type: 'status', band: 'connected', mic: 'live' },
      { type: 'memory', op: 'confirmed', item: budget },
      { type: 'memory', op: 'confirmed', item: nights },
      { type: 'memory', op: 'proposed', item: firm },
      say(1, 0, "It's $1,399 delivered."),
      ...decide(1, 'Pause', true, {
        reason: 'Over budget: $1,399 is above your $1,200 limit.',
        facts: [{ key: 'price_total', value: '1399', turn_id: 1, confirmed: true }],
        missing: ['return_nights'], conflicts: ['price_total 1399 > budget_total 1200'],
        memory_used: [ref(budget, t(1) + 120)],
      }),
      say(2, 1, "That's too high for me."),
      say(3, 0, 'I can do $1,150 with a 30-day return.'),
      ...decide(2, 'Pause', false, { // decided after turn 3 had already ended
        reason: 'Still over budget: no new price stated.',
        facts: [{ key: 'price_total', value: '1399', turn_id: 1, confirmed: true }],
        missing: ['return_nights'], conflicts: ['price_total 1399 > budget_total 1200'],
        memory_used: [ref(budget, t(2) + 150)],
      }),
      ...decide(3, 'Pause', true, {
        reason: 'Return policy too short: 30 days is under your 100-night minimum.',
        facts: [
          { key: 'price_total', value: '1150', turn_id: 3, confirmed: true },
          { key: 'return_nights', value: '30', turn_id: 3, confirmed: true },
        ],
        missing: [], conflicts: ['return_nights 30 < 100'],
        memory_used: [ref(budget, t(3) + 110), ref(nights, t(3) + 110)],
      }),
      say(4, 0, 'Fine, forget that return offer. $1,150 then.'),
      ...decide(4, 'Ask', true, {
        reason: 'Return policy not stated.',
        facts: [{ key: 'price_total', value: '1150', turn_id: 4, confirmed: true }],
        missing: ['return_nights'], conflicts: [],
        memory_used: [ref(budget, t(4) + 100), ref(nights, t(4) + 100)],
      }),
      say(5, 0, '$1,150, 100-night return, delivery included.'),
      ...decide(5, 'Proceed', true, {
        reason: 'All goals satisfied.',
        facts: [
          { key: 'price_total', value: '1150', turn_id: 5, confirmed: true },
          { key: 'return_nights', value: '100', turn_id: 5, confirmed: true },
          { key: 'delivery_included', value: 'yes', turn_id: 5, confirmed: true },
        ],
        missing: [], conflicts: [],
        memory_used: [ref(budget, t(5) + 90), ref(nights, t(5) + 90)],
      }),
      say(6, 1, "Great, then let's", false),
    ],
  };
}

export function lunchDemo(now = Date.now()): Demo {
  const t = (n: number) => now - MIN + n * 12_000;
  const veg: MemoryItem = { id: 'm4', key: 'diet', value: 'vegetarian', status: 'confirmed', source: 'user', saved_ms: now - 9 * 24 * 60 * MIN, last_used_ms: t(2), used_by: 2 };
  const places = (id: string, key: string, value: string, at: number) => ({ id, key, value, source: 'Maps/Places', retrieved_ms: at });
  return {
    session: {
      mode: 'lunch',
      goals: [
        { key: 'diet', value: 'vegetarian', confirmed: true },
        { key: 'time_total', value: '≤ 40 min', confirmed: true },
      ],
      lunch_options: { A: 'Sushi Kai', B: 'Verde Salads', C: 'Noodle Bar' },
    },
    events: [
      { type: 'status', band: 'connected', mic: 'live' },
      { type: 'memory', op: 'confirmed', item: veg },
      { type: 'transcript', turn_id: 1, speaker: 1, text: "I've only got forty minutes, where should we go?", final: true, time_ms: t(1) },
      {
        type: 'decision', id: 'l1', turn_id: 1, mode: 'lunch', cue: "Lunch('B')",
        reason: 'Verde Salads is vegetarian-friendly and the closest that fits 40 minutes.',
        facts: [{ key: 'time_total', value: '40 min', turn_id: 1, confirmed: true }],
        missing: [], conflicts: [],
        memory_used: [
          { id: veg.id, key: veg.key, value: veg.value, source: veg.source, retrieved_ms: t(1) + 80 },
          places('p-b', 'B · Verde Salads', '6 min walk · open · vegetarian menu', t(1) + 210),
          places('p-c', 'C · Noodle Bar', '9 min walk · open · 2 vegetarian mains', t(1) + 210),
          places('p-a', 'A · Sushi Kai', '4 min walk · open · no vegetarian mains', t(1) + 210),
        ],
        choice: { option: 'B', eligible: ['B', 'C'] },
        latency_ms: { transcript_to_decision: 410, decision_to_led: 45 }, time_ms: t(1) + 420,
      },
      { type: 'cue', turn_id: 1, cue: "Lunch('B')", shown: true, time_ms: t(1) + 465 },
      { type: 'transcript', turn_id: 2, speaker: 0, text: "Verde is closed for a private event today, and I can't do noodles.", final: true, time_ms: t(2) },
      {
        type: 'decision', id: 'l2', turn_id: 2, mode: 'lunch', cue: 'Off',
        reason: 'No option fits: B is closed, C was ruled out, A has no vegetarian mains.',
        facts: [
          { key: 'B_open', value: 'no (private event)', turn_id: 2, confirmed: true },
          { key: 'C_excluded', value: 'yes', turn_id: 2, confirmed: true },
        ],
        missing: [], conflicts: ['A: no vegetarian mains'],
        memory_used: [
          { id: veg.id, key: veg.key, value: veg.value, source: veg.source, retrieved_ms: t(2) + 70 },
          places('p-a', 'A · Sushi Kai', '4 min walk · open · no vegetarian mains', t(1) + 210),
        ],
        choice: { option: null, eligible: [] },
        latency_ms: { transcript_to_decision: 230 }, time_ms: t(2) + 240,
      },
      { type: 'cue', turn_id: 2, cue: 'Off', shown: true, time_ms: t(2) + 250 },
    ],
  };
}
