"""Python half of the bridge. Runs inside the Rust process (src/bridge.rs).

Rust sets `emit(text, speaker, is_final, purpose)`, `publish(json)` and `set_mode(on)` on
this module, then calls run() on its own thread and set_cue() whenever the backend has a
decision. Only this process connects to the band (one Bluetooth connection, AGENTS.md rule 7).
Gestures are detected here, where the band's events arrive; only the result crosses to Rust.

Decisions mode (triple tap toggles it; off at start):
  on:  green blinks 5 times, then a fresh Deepgram connection (speaker numbers start again)
       and the mic: every turn is sent, saved and decided on as one conversation.
  off: red blinks 5 times, the mic stops (nothing is heard, sent or saved), the light goes off.

Turns end on Deepgram's UtteranceEnd: a gap after the last recognised *word*, worked out from
the transcript's word timings, so background noise cannot hold a turn open the way it holds
open the audio silence detector (speech_final, ignored here).

Wave (decisions mode only): the band flashes white (speak now), and the next turn is the
user's updated context (purpose "context"); the UI asks to confirm it before it replaces
their active memory.
"""
import json
import os
import sys
import threading
import time
from collections import deque
from pathlib import Path

sys.stdout.reconfigure(line_buffering=True)  # embedded Python: print lines as they come
KIT = Path(__file__).resolve().parent.parent / "vexo-open-kit"
sys.path.insert(0, str(KIT / "examples" / "python"))  # speech.py (finds the library itself)

UTTERANCE_END_MS = 1000  # gap after the last word that ends a turn (Deepgram's minimum is 1000)
CUE_HOLD_S = 2.0         # the light only shows with the mic off; speech in this window is not heard

# ponytail: gesture thresholds are guesses until measured on band 48. VEXO_GESTURE_DEBUG=1
# prints every tap event and, once a second, the peak turning rate per axis.
TAPS = 3                 # taps that toggle decisions mode ...
TAP_WINDOW_MS = 1200     # ... within this long
DOUBLE_TAP_COUNTS = 2    # a `double-tap` event stands for this many taps (set 0 if the band
                         # also sends a `tap` for each of them)
TAP_THRESHOLD_G = 0.8    # the band's tap sensitivity (kit default 0.8; lower = lighter taps)
MODE_BLINKS = 5          # green 5 times = on, red 5 times = off
BLINK_PERIOD_MS = 400

WAVE_AXIS = "gz"         # turning rate that flips sign on each left/right swing
WAVE_DPS = 150           # a swing must turn faster than this (degrees/second)
WAVE_SWINGS = 5          # left-right three times = L, R, L, R, L: five swings of alternating sign
WAVE_WINDOW_MS = 3000    # all of them within this long
WAVE_ACK_S = 0.8         # white flash that says "speak now" (the mic is off while it shows)

band = None
ears = None
decisions_on = False
_context_next = False    # the next turn is spoken context, after a wave
_resume = None
_mode_lock = threading.Lock()

import speech  # noqa: E402  (after the path line above)


class Diarized(speech.DeepgramSpeech):
    """The kit's Deepgram engine with transcript-based turns and the speaker of the latest
    words (diarize=true). Empty text (silence, noise) is never passed on."""
    speaker = None

    def __init__(self, **options):
        super().__init__(diarize="true", utterance_end_ms=UTTERANCE_END_MS, **options)

    def _message(self, message):
        if message.get("type") == "UtteranceEnd":
            self._sentence_ends()  # emits only if words were recognised
            return
        alts = (message.get("channel") or {}).get("alternatives") or [{}]
        found = [w["speaker"] for w in alts[0].get("words") or [] if "speaker" in w]
        if found:
            self.speaker = max(set(found), key=found.count)
        super()._message(dict(message, speech_final=False))  # the audio VAD never ends a turn

    def _say(self, text, final):
        if (text or "").strip():
            super()._say(text, final)


class TapCounter:
    """TAPS taps within TAP_WINDOW_MS is a triple tap. The band only knows tap and double-tap."""

    def __init__(self, taps=TAPS, window_ms=TAP_WINDOW_MS):
        self.taps, self.window_ms = taps, window_ms
        self.seen = deque()

    def feed(self, t_ms, count=1):
        self.seen.extend([t_ms] * count)
        while self.seen and t_ms - self.seen[0] > self.window_ms:
            self.seen.popleft()
        if len(self.seen) < self.taps:
            return False
        self.seen.clear()
        return True


class WaveDetector:
    """Counts fast swings of alternating direction; WAVE_SWINGS of them within the window is a wave."""

    def __init__(self, dps=WAVE_DPS, swings=WAVE_SWINGS, window_ms=WAVE_WINDOW_MS):
        self.dps, self.swings, self.window_ms = dps, swings, window_ms
        self.peaks = deque()   # (t_ms, sign) of each swing
        self.quiet_until = 0

    def feed(self, t_ms, rate):
        if rate is None or abs(rate) < self.dps or t_ms < self.quiet_until:
            return False
        sign = 1 if rate > 0 else -1
        while self.peaks and t_ms - self.peaks[0][0] > self.window_ms:
            self.peaks.popleft()
        if self.peaks and self.peaks[-1][1] == sign:
            return False       # the same swing, still going
        self.peaks.append((t_ms, sign))
        if len(self.peaks) < self.swings:
            return False
        self.peaks.clear()
        self.quiet_until = t_ms + 1500  # one wave, one event
        return True


def _now_ms():
    return int(time.time() * 1000)


def _emit(text, final):
    global _context_next
    if not decisions_on:
        return                 # off: nothing is sent or saved
    purpose = "context" if _context_next else "conversation"
    if final and _context_next:
        _context_next = False
    emit(text, ears.speaker if ears else None, final, purpose)  # noqa: F821  (set by Rust)


def toggle_decisions():
    """Triple tap: decisions mode on or off. Hands the new mode to Rust (set_mode)."""
    global decisions_on, ears, _context_next
    with _mode_lock:
        if not decisions_on:
            if band:
                try:
                    fresh = Diarized()
                    fresh.start(_emit)   # a new connection: speaker numbers start again
                except speech.SpeechError as e:
                    print("Decisions mode stays off:", e)
                    _flash("#ff0000", "blink", MODE_BLINKS * BLINK_PERIOD_MS / 1000, then=band.led_off)
                    return
                ears = fresh
            decisions_on = True
            set_mode(True)  # noqa: F821  (Rust: new conversation, UI opens it)
            print("Decisions mode ON: listening.")
            _blink("#00ff00", then=_mic_on)
        else:
            _context_next = False
            print("Decisions mode OFF: not listening.")
            if band:
                band.configure(audio=False, mic_level=False)  # nothing new is heard from here
            _blink("#ff0000", then=band.led_off if band else None)
            if ears:
                ears.stop()            # words already spoken are still sent, before the mode ends
            decisions_on, ears = False, None
            set_mode(False)  # noqa: F821


def on_wave():
    """Wave seen: words so far finish the conversation turn, then the next turn is context."""
    global _context_next
    if not decisions_on:
        return
    if ears:
        ears._sentence_ends()
    _context_next = True
    publish(json.dumps({"type": "gesture", "name": "wave", "time_ms": _now_ms()}))  # noqa: F821
    print("Wave: say the updated context now.")
    if band:
        _flash("#ffffff", "solid", WAVE_ACK_S, then=_mic_on)


def run(band_number):
    global band
    if band_number is None:
        print("TYPED input (no band, no microphone): one line per turn. /triple = triple tap, /wave = wave.")
        for line in sys.stdin:
            line = line.strip()
            if line == "/triple":
                toggle_decisions()
            elif line == "/wave":
                on_wave()
            elif line:
                if not decisions_on:
                    print("(decisions mode is off: type /triple first)")
                _emit(line, True)
        return

    from vexo_open.sync import Band

    band = Band.connect(band_number, auto_reconnect=True)
    debug = bool(os.environ.get("VEXO_GESTURE_DEBUG"))
    taps, wave = TapCounter(), WaveDetector()
    peaks = {"gx": 0, "gy": 0, "gz": 0, "t": 0}

    def on_tap(e, count):
        if debug:
            print("gesture debug:", e.kind)
        if taps.feed(_now_ms(), count):
            threading.Thread(target=toggle_decisions, daemon=True).start()  # keep the handler short

    def on_motion(m):
        if debug:  # calibration: the fastest turning per axis, once a second
            for axis in ("gx", "gy", "gz"):
                peaks[axis] = max(peaks[axis], abs(getattr(m, axis) or 0))
            if m.t - peaks["t"] > 1000:
                print("gesture debug: peak deg/s  gx %(gx)4.0f  gy %(gy)4.0f  gz %(gz)4.0f" % peaks)
                peaks.update(gx=0, gy=0, gz=0, t=m.t)
        if decisions_on and wave.feed(m.t, getattr(m, WAVE_AXIS)):
            threading.Thread(target=on_wave, daemon=True).start()

    band.on("audio", lambda a: ears and ears.feed(a))
    band.on("tap", lambda e: on_tap(e, 1))
    band.on("double-tap", lambda e: on_tap(e, DOUBLE_TAP_COUNTS))
    band.on("motion", on_motion)
    band.set_events(kinds=["tap", "double-tap"], tap_threshold=TAP_THRESHOLD_G)
    band.configure(events=True, motion=50)  # mic stays off until decisions mode is on
    print("Connected to %s. Triple-tap the band to start decisions mode." % band.name)
    while True:
        band.wait(1)


def _mic_on():
    if band and decisions_on:
        band.configure(audio=True)  # solid blue: the microphone is on


def _blink(hex_colour, then):
    if band:
        _flash(hex_colour, "blink", MODE_BLINKS * BLINK_PERIOD_MS / 1000, then=then)
    else:
        print("TYPED light (no band): %s blinks %d times" % (hex_colour, MODE_BLINKS))


def set_cue(hex_colour, pattern):
    """Mic off (the band refuses LED commands otherwise), light for CUE_HOLD_S, mic back on."""
    if band is None:
        print("TYPED cue (no band):", hex_colour or "off", pattern)
        return
    if not decisions_on or _mode_lock.locked():
        return                 # off, or switching: the mode's blinks win
    if ears:
        ears._sentence_ends()  # words heard so far become a turn; with no sound, no UtteranceEnd would come
    _flash(hex_colour, pattern, CUE_HOLD_S, then=_mic_on)


def _flash(hex_colour, pattern, seconds, then):
    """Mic off, the light for `seconds`, then `then()` (mic back on, or light off)."""
    global _resume
    if _resume:
        _resume.cancel()
    band.configure(audio=False, mic_level=False)
    if hex_colour:
        band.set_led(hex_colour, pattern=pattern, period=BLINK_PERIOD_MS if pattern == "blink" else 600)
    else:
        band.led_off()
    _resume = threading.Timer(seconds, then)
    _resume.daemon = True
    _resume.start()
