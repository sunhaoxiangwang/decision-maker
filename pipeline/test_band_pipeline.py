"""Turn logic without a band or Deepgram:  .venv/bin/python3 pipeline/test_band_pipeline.py"""
import sys
from types import SimpleNamespace
from unittest.mock import patch
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import band_pipeline as bp
import importlib.util
if bp.KIT.exists():  # a clean checkout without the kit still runs the kit-free checks
    assert importlib.util.find_spec("vexo_open"), "run() imports vexo_open before speech is ever imported"
try:
    import speech
except ModuleNotFoundError:
    speech = None

if speech is not None:
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


else:
    print('Speech adapter checks require scripts/setup.sh; running kit-free gesture checks.')

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

# Both modes start off. Three waves toggle continuous capture, including with decisions off.
sent, modes, memory_modes = [], [], []
bp.emit = lambda text, speaker, final, purpose: sent.append((text, final, purpose))
bp.set_mode = modes.append
bp.set_memory_mode = memory_modes.append
bp.ears = None
bp._emit("off", True)
assert sent == []
bp.on_three_waves()
assert bp.memory_on and not bp.decisions_on
bp._emit("Budget is now $1,300.", True)
bp._emit("And delivery", False)
bp._emit("And delivery must be included.", True)
assert all(s[2] == "context" for s in sent)
bp.on_three_waves()
assert not bp.memory_on and memory_modes == [True, False]
n = len(sent)
bp._emit("after memory off", True)
assert len(sent) == n
bp.toggle_decisions()
assert bp.decisions_on and modes == [True]
bp._emit("conversation", True)
assert sent[-1][2] == "conversation"
bp.on_three_waves()
assert not bp.set_cue("#00ff00", "solid")
bp._emit("context one", True)
bp._emit("context two", True)
assert [s[2] for s in sent[-2:]] == ["context", "context"]
bp.on_three_waves()
bp._emit("conversation again", True)
assert sent[-1][2] == "conversation"
assert bp.set_cue("#00ff00", "solid")
bp.on_three_waves()
bp.toggle_decisions()  # stopping decisions also stops active memory capture
assert not bp.decisions_on and not bp.memory_on
assert modes == [True, False]
assert not bp.set_cue("#00ff00", "solid")
print("memory toggle ok")

# ---- triple tap ---------------------------------------------------------------------------
t = bp.TapCounter(taps=3, window_ms=1200)
assert not t.feed(0) and not t.feed(300) and t.feed(600)       # tap tap tap
assert not t.feed(5000) and t.feed(5300, count=2)               # tap + double-tap
assert not t.feed(9000) and not t.feed(9800) and not t.feed(11000)  # too slow
print("triple tap ok")


# Exercise the actual registered tap/motion handlers, mic/LED ordering, and final drain.
class FakeBand:
    def __init__(self):
        self.handlers, self.calls = {}, []
        self.audio = False

    def on(self, event, callback):
        self.handlers[event] = callback

    def configure(self, **options):
        self.calls.append(("configure", options))
        self.audio = options.get("audio", self.audio)

    def set_events(self, **options):
        self.calls.append(("events", options))

    def set_led(self, colour, **options):
        assert not self.audio, "LED commands require mic off"
        self.calls.append(("led", colour, options))

    def led_off(self):
        assert not self.audio
        self.calls.append(("off",))


class FakeEars:
    speaker = 0

    def __init__(self):
        self.pending = ""

    def start(self, callback):
        self.callback = callback

    def stop(self):
        if self.pending:
            self.callback(self.pending, True)
        self.pending = ""

    def _sentence_ends(self):
        self.stop()


class FakeTimer:
    def __init__(self, seconds, callback):
        self.seconds, self.callback = seconds, callback

    def start(self):
        pass

    def cancel(self):
        pass  # deliberately allow stale callbacks to run: production must reject them


class ImmediateThread:
    def __init__(self, target, **options):
        self.target = target

    def start(self):
        self.target()


bp.band = fake = FakeBand()
sent.clear()
modes.clear()
memory_modes.clear()
with patch.object(bp, "Diarized", FakeEars), patch.object(bp.threading, "Timer", FakeTimer), patch.object(bp.threading, "Thread", ImmediateThread):
    bp._watch_gestures()
    assert not fake.audio
    assert modes == []  # subscribing does not activate decisions
    tap = lambda kind: fake.handlers[kind](SimpleNamespace(kind=kind))
    with patch.object(bp, "_now_ms", side_effect=[0, 300, 600, 2000, 2300]):
        tap("tap")
        tap("tap")
        assert not bp.decisions_on
        tap("tap")
        assert bp.decisions_on and modes == [True]
        assert ("led", "#00ff00", {"pattern": "blink", "period": 400}) in fake.calls
        green_timer = bp._resume
        assert green_timer.seconds == 2.0  # five 400-ms blinks
        assert not fake.audio
        green_timer.callback()
        assert fake.audio

        prior = len(sent)
        prior_leds = len([c for c in fake.calls if c[0] == "led"])
        bp.ears.pending = "a newer offer invalidates the old green"
        assert not bp.set_cue("#00ff00", "solid", validate=lambda: len(sent) == prior)
        assert sent[-1][0] == "a newer offer invalidates the old green"
        assert len([c for c in fake.calls if c[0] == "led"]) == prior_leds
        assert fake.audio  # stale cue resumes listening without showing the old colour
        with patch.object(fake, "set_led", side_effect=RuntimeError("LED command failed")):
            try:
                bp.set_cue("#00ff00", "solid")
                assert False, "LED failure must propagate"
            except RuntimeError:
                pass
        assert fake.audio and bp._resume is None
        bp.ears.pending = "last conversation words"
        def wave(start, first=300):  # one wave: five alternating swings, 200 ms apart
            for i in range(5):
                fake.handlers["motion"](SimpleNamespace(t=start + i * 200, gz=first if i % 2 == 0 else -first))

        def three_waves(start):  # detected at start+0.8 s, +3.2 s, +5.6 s; direction does not matter
            for k, first in enumerate([300, -300, 300]):
                wave(start + k * 2400, first)

        wave(0)
        wave(2400)  # two waves (detected at 0.8 s and 3.2 s): nothing changes, nothing is captured as context
        assert not bp.memory_on and memory_modes == [] and bp.decisions_on
        assert sent[-1][2] == "conversation"
        three_waves(12000)  # the two earlier waves are > 8 s old: only the third of these toggles
        assert bp.memory_on and memory_modes == [True]
        assert sent[-1] == ("last conversation words", True, "conversation")
        bp._resume.callback()
        assert fake.audio
        bp._emit("memory first", True)
        bp.ears.pending = "memory last words"
        three_waves(22000)
        assert not bp.memory_on and memory_modes == [True, False]
        assert sent[-1] == ("memory last words", True, "context")
        bp._resume.callback()
        assert fake.audio  # decisions resume after memory stops
        bp.ears.pending = "last words before off"
        tap("tap")
        assert bp.decisions_on
        tap("double-tap")  # one plus two taps also toggles
        assert not bp.decisions_on and modes == [True, False]
        assert sent[-1] == ("last words before off", True, "conversation")
        assert ("led", "#ff0000", {"pattern": "blink", "period": 400}) in fake.calls
        assert bp._resume.seconds == 2.0
        green_timer.callback()  # old timer must not turn the mic back on
        assert not fake.audio
        bp._resume.callback()
        assert fake.calls[-1] == ("off",)

    three_waves(32000)  # memory starts even with decisions off
    assert bp.memory_on and not bp.decisions_on
    bp._resume.callback()
    assert fake.audio
    bp.ears.pending = "standalone memory"
    three_waves(42000)
    assert not bp.memory_on and not bp.decisions_on
    assert sent[-1] == ("standalone memory", True, "context")
    bp._resume.callback()
    assert not fake.audio and fake.calls[-1] == ("off",)

    def fail_start(self, callback):
        raise RuntimeError("test connection failure")

    with patch.object(FakeEars, "start", fail_start):
        bp.toggle_decisions()
        assert not bp.decisions_on and bp.ears is None
        bp.on_three_waves()
        assert not bp.memory_on and bp.ears is None

print("registered gestures, LEDs, drain and failures ok")

# ---- three waves --------------------------------------------------------------------------
# The registered motion handler: only THREE_WAVES detected waves within THREE_WAVE_WINDOW_MS
# toggle memory. Fewer, or spread wider, never call set_memory_mode, so no speech is captured
# as context (the only purpose Rust sends to the copilot as trigger three_waves).
assert (bp.THREE_WAVES, bp.THREE_WAVE_WINDOW_MS) == (3, 8000)
bp.band = fake = FakeBand()
bp.decisions_on, bp.memory_on, bp._capture_failed, bp.ears = False, False, False, None
memory_modes.clear()
toggles = []
with patch.object(bp, "Diarized", FakeEars), patch.object(bp.threading, "Timer", FakeTimer), \
        patch.object(bp.threading, "Thread", ImmediateThread), \
        patch.object(bp, "on_three_waves", lambda: toggles.append(True)):
    bp._watch_gestures()

    def wave_at(start, first=300):
        for i in range(5):
            fake.handlers["motion"](SimpleNamespace(t=start + i * 200, gz=first if i % 2 == 0 else -first))

    wave_at(0)                                   # wave 1 (detected at 800)
    assert toggles == []
    wave_at(2400, -300)                          # wave 2, starting the other way (3200)
    assert toggles == []
    wave_at(10000)                               # wave 3 at 10800: wave 1 is > 8 s old
    assert toggles == []
    wave_at(12400, -300)                         # 3200, 10800, 13200 ... 3200 has expired too
    assert toggles == []
    wave_at(14800)                               # 10800, 13200, 15600: three within 8 s
    assert toggles == [True]
    wave_at(30000, -300)
    wave_at(32400, 300)
    assert toggles == [True]                     # two more: nothing
    wave_at(34800, -300)                         # three more: toggles again (off)
    assert toggles == [True, True]
    wave_at(50000)                               # the count restarts after each toggle
    assert toggles == [True, True]
    for start in (60000, 62400, 64800):          # slow single swings are not waves at all
        fake.handlers["motion"](SimpleNamespace(t=start, gz=300))
    assert toggles == [True, True]
assert memory_modes == [] and not bp.memory_on   # (on_three_waves was stubbed: counting only)

# The same counts through the real toggle: one or two waves leave capture off and emit nothing.
sent.clear()
bp.band = fake = FakeBand()
with patch.object(bp, "Diarized", FakeEars), patch.object(bp.threading, "Timer", FakeTimer), \
        patch.object(bp.threading, "Thread", ImmediateThread):
    bp._watch_gestures()
    wave_at(0)
    wave_at(2400, -300)
    bp._emit("said after two waves", True)
    assert not bp.memory_on and memory_modes == [] and sent == []
    wave_at(4800)
    assert bp.memory_on and memory_modes == [True]
    bp._resume.callback()
    bp._emit("said after three waves", True)
    assert sent == [("said after three waves", True, "context")]
    for k, start in enumerate((20000, 22400)):
        wave_at(start, 300 if k % 2 else -300)
        assert bp.memory_on and memory_modes == [True]  # one or two waves do not stop it
    wave_at(24800, -300)
    assert not bp.memory_on and memory_modes == [True, False]
    bp._resume.callback()
print("three waves toggle, one or two do not, the window and any direction ok")

if speech is not None:
    # Continuous recognized speech is segmented without retaining all past final text.
    bp.band = None
    out = []
    bounded = bp.Diarized()
    speech.Speech.start(bounded, lambda text, final: out.append((text, final)))
    bounded._parts = []
    for _ in range(30):
        bounded._message({"type": "Results", "is_final": True,
                          "channel": {"alternatives": [{"transcript": "word " * 400}]}})
    bounded._sentence_ends()
    assert bounded.finals == []
    assert all(len(text.encode("utf-8")) <= bp.MAX_TEXT_BYTES for text, final in out if final)
    print("speech segmentation and stale cue regression ok")


# Failed handoff closes both modes rather than blocking or silently losing future turns.
bp.band, bp.ears = None, None
bp.decisions_on, bp.memory_on, bp._capture_failed = True, False, False
bp.emit = lambda *_: False
with patch.object(bp.threading, "Thread", ImmediateThread):
    bp._emit("rejected by full consumer", True)
assert not bp.decisions_on and not bp.memory_on and bp._capture_failed
print("bounded consumer failure stops capture ok")


if speech is not None:
    # Test timeout without waiting for a real socket or contacting Deepgram.
    probe = bp.Diarized()
    checks = []
    probe._done = SimpleNamespace(wait=lambda _: False)
    probe._pending_lock = bp.threading.Lock()
    probe._pending_sends = {1: bp.time.monotonic() - 10}
    probe._ws = SimpleNamespace(sock=SimpleNamespace(shutdown=lambda _: checks.append('socket aborted')))
    with patch.object(bp, '_fail_capture', lambda *args, **kw: checks.append('capture failed')):
        probe._write_watchdog()
    assert checks == ['socket aborted', 'capture failed']
    print('speech write deadline ok')
