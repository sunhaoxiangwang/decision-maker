"""Turn logic without a band or Deepgram:  .venv/bin/python3 pipeline/test_band_pipeline.py"""
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import band_pipeline as bp
import speech

out = []
ears = bp.Diarized()
speech.Speech.start(ears, lambda text, final: out.append((text, ears.speaker, final)))  # no network
ears._parts = []


def results(text, final, speaker=0, speech_final=False):
    words = [{"word": w, "speaker": speaker} for w in text.split()]
    ears._message({"type": "Results", "is_final": final, "speech_final": speech_final,
                   "channel": {"alternatives": [{"transcript": text, "words": words}]}})


results("", False)                                   # noise: nothing passed on
results("", True, speech_final=True)                 # audio VAD on silence: nothing
assert out == [], out
results("I can do", False)
results("I can do 1150", True, speech_final=True)    # speech_final must NOT end the turn
assert all(not f for _, _, f in out), out
results("with a thirty day return", True, speaker=1)
ears._message({"type": "UtteranceEnd", "last_word_end": 3.1})
assert out[-1] == ("I can do 1150 with a thirty day return", 1, True), out[-1]
n = len(out)
ears._message({"type": "UtteranceEnd"})              # no new words: no empty turn
assert len(out) == n
print("ok")


# ---- wave gesture -------------------------------------------------------------------------
w = bp.WaveDetector(dps=150, swings=5, window_ms=3000)
swing = lambda t, r: w.feed(t, r)
assert not any(swing(t, r) for t, r in [(0, 300), (50, 320), (300, -280), (600, 290)])  # 3 swings, one held long
assert not swing(700, 100)                  # too slow to count
assert not swing(900, -300)                 # 4
assert swing(1200, 310)                     # 5 alternating swings within 3 s: a wave
assert not swing(1500, -300)                # quiet for 1.5 s after a wave
w2 = bp.WaveDetector(dps=150, swings=5, window_ms=3000)
assert not any(w2.feed(t, r) for t, r in [(0, 300), (2000, -300), (4000, 300), (6000, -300), (8000, 300)])  # too slow overall

# A wave makes the next final turn context, then conversation again.
sent = []
bp.emit = lambda text, speaker, final, purpose: sent.append((text, final, purpose))
bp.publish = lambda js: sent.append(("event", js))
modes = []
bp.set_mode = modes.append
bp.ears = None
bp._emit("not in decisions mode", True)     # off: dropped
assert sent == []
bp.toggle_decisions()                       # triple tap: on (no band in this test)
assert bp.decisions_on and modes == [True]
bp._emit("before", True)
bp.on_wave()
bp._emit("my budget is now", False)
bp._emit("my budget is now 1300", True)
bp._emit("ok so", True)
assert [s[2] for s in sent if s[0] != "event"] == ["conversation", "context", "context", "conversation"], sent
assert any(s[0] == "event" and '"wave"' in s[1] for s in sent)
bp.toggle_decisions()                       # triple tap again: off
assert not bp.decisions_on and modes == [True, False]
n = len(sent)
bp.on_wave()
bp._emit("after off", True)
assert len(sent) == n                       # off: no wave, no turns
print("wave ok")

# ---- triple tap ---------------------------------------------------------------------------
t = bp.TapCounter(taps=3, window_ms=1200)
assert not t.feed(0) and not t.feed(300) and t.feed(600)       # tap tap tap
assert not t.feed(5000) and t.feed(5300, count=2)               # tap + double-tap
assert not t.feed(9000) and not t.feed(9800) and not t.feed(11000)  # too slow
print("triple tap ok")
