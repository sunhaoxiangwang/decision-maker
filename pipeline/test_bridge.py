"""Python → embedded Rust → SSE → frontend reducer, with no band or API keys.

Run after cargo build: .venv/bin/python3 pipeline/test_bridge.py
"""
import http.client
import json
import os
from pathlib import Path
import queue
import socket
import subprocess
import threading
import time

ROOT = Path(__file__).resolve().parent.parent
with socket.socket() as reservation:
    reservation.bind(("127.0.0.1", 0))
    port = reservation.getsockname()[1]

process = subprocess.Popen(
    [str(ROOT / "target/debug/decision-maker"), "--typed", "--cue"],
    cwd=ROOT, env=dict(os.environ, BRIDGE_PORT=str(port)),
    stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
)
events, incoming = [], queue.Queue()


def subscribe():
    connection = http.client.HTTPConnection("127.0.0.1", port, timeout=5)
    connection.request("GET", "/events", headers={"Origin": "http://localhost:3000"})
    response = connection.getresponse()
    assert response.status == 200
    return connection, response


def read_events(response):
    for line in response:
        if line.startswith(b"data: "):
            incoming.put(json.loads(line[6:]))


def expect(kind, **fields):
    deadline = time.monotonic() + 5
    while True:
        event = incoming.get(timeout=max(0.01, deadline - time.monotonic()))
        events.append(event)
        if event["type"] == kind and all(event.get(k) == v for k, v in fields.items()):
            return event
        assert time.monotonic() < deadline, (kind, fields, events)


def send(line):
    process.stdin.write(line + "\n")
    process.stdin.flush()


try:
    deadline = time.monotonic() + 5
    while True:
        try:
            connection, response = subscribe()
            break
        except OSError:
            assert process.poll() is None, process.stdout.read()
            assert time.monotonic() < deadline, "bridge did not start"
            time.sleep(0.02)
    threading.Thread(target=read_events, args=(response,), daemon=True).start()
    expect("mode", decisions=False)
    expect("gesture", memory=False)
    send("dropped while off")
    send("/triple")
    expect("mode", decisions=True, conversation=1)
    send("First conversation.")
    first = expect("transcript", text="First conversation.", purpose="conversation")
    expect("cue", turn_id=first["turn_id"], shown=True)

    send("/wave")
    expect("gesture", memory=True)
    replay_connection, replay = subscribe()
    assert json.loads(replay.readline()[6:])["decisions"] is True
    assert replay.readline() == b"\n"
    assert json.loads(replay.readline()[6:])["memory"] is True
    replay_connection.close()
    for text in ["Budget: $1,300.", "Delivery must be included!"]:
        send(text)
        expect("transcript", text=text, purpose="context")
    send("/wave")
    expect("gesture", memory=False)
    send("Conversation resumes.")
    resumed = expect("transcript", text="Conversation resumes.", purpose="conversation")
    expect("cue", turn_id=resumed["turn_id"], shown=True)
    send("/triple")
    expect("mode", decisions=False, conversation=1)

    send("/wave")
    expect("gesture", memory=True)
    send("Standalone memory.")
    expect("transcript", text="Standalone memory.", purpose="context")
    send("/wave")
    expect("gesture", memory=False)
    send("also dropped while off")
    send("/triple")
    expect("mode", decisions=True, conversation=2)
    send("Second conversation.")
    second = expect("transcript", text="Second conversation.", purpose="conversation")
    expect("cue", turn_id=second["turn_id"], shown=True)
    send("/triple")
    expect("mode", decisions=False, conversation=2)

    transcripts = [e for e in events if e["type"] == "transcript"]
    assert len(transcripts) == 6, transcripts
    context_ids = {e["turn_id"] for e in transcripts if e["purpose"] == "context"}
    assert not any(e["type"] == "cue" and e["turn_id"] in context_ids for e in events)
    assert [e["conversation"] for e in transcripts if e["purpose"] == "conversation"] == [1, 1, 2]

    # Fold the actual Rust event stream through the same reducer used in the UI.
    check = subprocess.run(["node", "--input-type=module", "-e", """
      import assert from 'node:assert/strict';
      import { initial, reducer, visibleTurns } from './web/lib/store.ts';
      let input = ''; for await (const chunk of process.stdin) input += chunk;
      const state = JSON.parse(input).reduce(reducer, initial);
      assert.equal(state.decisionsOn, false);
      assert.equal(state.current, null);
      assert.deepEqual(visibleTurns(state), []);
      assert.deepEqual(Object.values(state.conversations).map(c => c.turns.map(t => t.text)),
        [['First conversation.', 'Conversation resumes.'], ['Second conversation.']]);
      assert.deepEqual(Object.values(state.memory).map(m => m.text),
        ['Budget: $1,300.\\nDelivery must be included!', 'Standalone memory.']);
      assert.equal(state.context, null);
      const saved = JSON.parse(JSON.stringify({ conversations: state.conversations, memory: state.memory }));
      const restored = reducer(initial, { type: 'load', saved });
      assert.deepEqual(restored.conversations, state.conversations);
      assert.deepEqual(restored.memory, state.memory);
    """], cwd=ROOT, input=json.dumps(events), text=True, capture_output=True)
    assert check.returncode == 0, check.stderr
    process.stdin.close()
    process.wait(timeout=5)
    assert process.returncode == 0, process.stdout.read()
    print("Python → Rust → SSE → frontend: decisions, wave memory, history and reload passed")
finally:
    if process.poll() is None:
        process.terminate()
        process.wait(timeout=5)
