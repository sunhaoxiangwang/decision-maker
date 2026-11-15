"""Python half of the bridge. Runs inside the Rust process (src/bridge.rs).

Rust sets `emit(text, speaker, is_final, purpose)`, `set_memory_mode(on)` and `set_mode(on)` on
this module, then calls run() on its own thread and set_cue() whenever the backend has a
decision. Only this process connects to the band (one Bluetooth connection, AGENTS.md rule 7).
Gestures are detected here, where the band's events arrive; only the result crosses to Rust.

Decisions mode (triple tap toggles it; off at start):
  on:  a fresh Deepgram connection (speaker numbers start again), green blinks 5 times,
       then the mic: conversation turns are sent and saved for decisions.
  off: red blinks 5 times, the mic stops (nothing is heard, sent or saved), the light goes off.

Turns end on Deepgram's UtteranceEnd: a gap after the last recognised *word*, worked out from
the transcript's word timings, so background noise cannot hold a turn open the way it holds
open the audio silence detector (speech_final, ignored here).

Three waves (THREE_WAVES within THREE_WAVE_WINDOW_MS, any direction) toggle memory capture,
even with decisions off; one or two waves do nothing. All turns until the next three waves are
context, never decisions. The UI saves each finished turn as given, until capture stops. Only
band context is sent to the copilot as trigger `three_waves`: typed `/wave` simulates the
gesture and Rust (src/main.rs) does not submit typed context.
"""
import json
import os
import socket
import sys
import threading
import time
from collections import deque
from pathlib import Path

sys.stdout.reconfigure(line_buffering=True)  # embedded Python: print lines as they come
KIT = Path(__file__).resolve().parent.parent / "vexo-open-kit"
sys.path.insert(0, str(KIT / "examples" / "python"))  # speech.py (finds the library itself)
sys.path.insert(0, str(KIT / "python"))  # vexo_open: run() needs it before speech is imported

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
THREE_WAVES = 3          # waves (as WaveDetector counts them, any direction) that toggle memory ...
THREE_WAVE_WINDOW_MS = 8000  # ... first to last within this long. Each wave takes ~1-1.5 s and is
                         # followed by WaveDetector's 1.5 s quiet time, so three deliberate waves
                         # span ~5-6.5 s; 8 s leaves margin while stray waves still expire.

band = None
ears = None
decisions_on = False
memory_on = False
_capture_failed = False
_mic_state = "off"
_band_state = "off"
_resume = None
_mode_lock = threading.Lock()

MAX_TEXT_BYTES = 8192
MAX_UTTERANCE_S = 30
SPEECH_WRITE_DEADLINE_S = 5
_speech_lock = threading.RLock()


def Diarized(**options):
    # Typed input and gesture tests do not require the external kit or Bluetooth packages.
    try:
        import speech
    except ModuleNotFoundError as e:
        raise RuntimeError("Band speech requires scripts/setup.sh; see README.md") from e

    class BoundedSpeech(speech.DeepgramSpeech):
        speaker = None

        def __init__(self):
            super().__init__(diarize="true", utterance_end_ms=UTTERANCE_END_MS, **options)
            self._utterance_started = time.monotonic()

        def start(self, on_text=None):
            super().start(on_text)
            original_send = self._ws.send
            self._pending_sends = {}
            self._pending_lock = threading.Lock()

            def send(message, opcode=None):
                identity = threading.get_ident()
                with self._pending_lock:
                    self._pending_sends[identity] = time.monotonic()
                try:
                    return original_send(message, opcode)
                finally:
                    with self._pending_lock:
                        self._pending_sends.pop(identity, None)

            self._ws.send = send
            threading.Thread(target=self._write_watchdog, daemon=True).start()
            return self

        def _write_watchdog(self):
            while not self._done.wait(0.1):
                with self._pending_lock:
                    oldest = min(self._pending_sends.values(), default=time.monotonic())
                if time.monotonic() - oldest > SPEECH_WRITE_DEADLINE_S:
                    # Do not acquire WebSocket's send lock: that is what may be blocked.
                    try:
                        self._ws.sock.shutdown(socket.SHUT_RDWR)
                    except OSError:
                        pass
                    _fail_capture("Speech service write timed out; capture stopped.", source=self)
                    return

        def _read(self):
            try:
                super()._read()
            finally:
                if not self._stopping and self.error:
                    _fail_capture("Speech service disconnected; capture stopped.", source=self)

        def _message(self, message):
            with _speech_lock:
                if message.get("type") == "UtteranceEnd":
                    self._sentence_ends()
                    return
                alts = (message.get("channel") or {}).get("alternatives") or [{}]
                words = alts[0].get("words") or []
                found = [w["speaker"] for w in words if "speaker" in w]
                if found:
                    self.speaker = max(set(found), key=found.count)
                piece = str(alts[0].get("transcript") or "")
                if message.get("type") == "Results" and message.get("is_final"):
                    if len(piece.encode("utf-8")) > MAX_TEXT_BYTES:
                        self._sentence_ends()
                        for start in range(0, len(piece), MAX_TEXT_BYTES // 4):
                            self._say(piece[start:start + MAX_TEXT_BYTES // 4], True)
                        return
                    if (len(" ".join(self._parts + [piece]).encode("utf-8")) > MAX_TEXT_BYTES
                            or time.monotonic() - self._utterance_started >= MAX_UTTERANCE_S):
                        self._sentence_ends()
                # The audio VAD never ends a turn. UtteranceEnd, time or size does.
                super()._message(dict(message, speech_final=False))

        def _sentence_ends(self):
            with _speech_lock:
                super()._sentence_ends()
                self._utterance_started = time.monotonic()

        def _say(self, text, final):
            text = (text or "").strip()
            if not text:
                return
            if not final and len(text.encode("utf-8")) > MAX_TEXT_BYTES:
                text = text.encode("utf-8")[:MAX_TEXT_BYTES].decode("utf-8", errors="ignore")
            # The UI owns history; the speech adapter retains no lifetime finals list.
            if self.on_text:
                self.on_text(text, final)

    return BoundedSpeech()


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


def _status(mic=None, link=None):
    global _mic_state, _band_state
    if mic is not None:
        _mic_state = mic
    if link is not None:
        _band_state = link
    callback = globals().get("publish")
    if callback:
        callback(json.dumps({"type": "status", "band": _band_state, "mic": _mic_state}, separators=(",", ":")))


def _idle():
    if band:
        band.led_off()
    _status("off")


def _now_ms():
    return int(time.time() * 1000)


def _emit(text, final):
    global decisions_on, memory_on, _capture_failed
    with _speech_lock:
        if not decisions_on and not memory_on:
            return
        purpose = "context" if memory_on else "conversation"
        accepted = emit(text, ears.speaker if ears else None, final, purpose)  # noqa: F821
        if accepted is False:
            _fail_capture()


def _fail_capture(message=None, source=None):
    global decisions_on, memory_on, _capture_failed
    with _speech_lock:
        if source is not None and ears is not source:
            return
        if _capture_failed:
            return
        decisions_on, memory_on, _capture_failed = False, False, True
        if message and globals().get("publish"):
            publish(json.dumps({"type": "gap", "message": message}))  # noqa: F821
        set_mode(False)  # noqa: F821
        set_memory_mode(False)  # noqa: F821
        threading.Thread(target=_stop_after_failure, daemon=True).start()


def _stop_after_failure():
    with _mode_lock:
        if not _capture_failed:
            return
        _stop_listening()
        if band:
            band.led_off()


def _start_listening():
    global ears
    if band:
        try:
            fresh = Diarized()
            fresh.start(_emit)
            if getattr(fresh, "error", None):
                fresh.stop()
                raise RuntimeError("Speech connection closed before capture started")
            ears = fresh
        except Exception as e:
            print("Listening stays off:", type(e).__name__, "— check speech setup/key/connection (README.md).")
            return False
    return True


def _stop_listening():
    """Drain the old connection before changing the transcript's purpose or mode."""
    global ears, _resume
    if _resume:
        _resume.cancel()
        _resume = None
    if band:
        band.configure(audio=False, mic_level=False)
    _status("off")
    if ears:
        try:
            ears.stop()
        except Exception as e:
            print("Listening stopped:", type(e).__name__)
        finally:
            ears = None


def toggle_decisions():
    """Triple tap: decisions mode on or off. Hands the new mode to Rust (set_mode)."""
    global decisions_on, memory_on, _capture_failed
    with _mode_lock:
        _capture_failed = False
        _stop_listening()
        if _capture_failed:
            return
        if memory_on:
            memory_on = False
            set_memory_mode(False)  # noqa: F821  (Rust: save the captured transcript)
        if not decisions_on:
            if not _start_listening():
                if band:
                    _flash("#ff0000", "blink", MODE_BLINKS * BLINK_PERIOD_MS / 1000, then=_idle)
                return
            decisions_on = True
            set_mode(True)  # noqa: F821  (Rust: new conversation, UI opens it)
            print("Decisions mode ON: listening.")
            _blink("#00ff00", then=_mic_on)
        else:
            decisions_on = False
            set_mode(False)  # noqa: F821  (drained turns arrived before this event)
            print("Decisions mode OFF: not listening.")
            _blink("#ff0000", then=_idle if band else None)


def on_three_waves():
    """Three waves (or typed /wave, simulated) start/stop memory capture; drain before each boundary."""
    global memory_on, decisions_on, _capture_failed
    with _mode_lock:
        _capture_failed = False
        _stop_listening()
        if _capture_failed:
            return
        if memory_on:
            memory_on = False
            set_memory_mode(False)  # noqa: F821
            print("Memory capture OFF: transcript saved.")
            if decisions_on and not _start_listening():
                decisions_on = False
                set_mode(False)  # noqa: F821
        elif _start_listening():
            memory_on = True
            set_memory_mode(True)  # noqa: F821
            print("Memory capture ON: wave three times again to save and stop.")
        else:
            if decisions_on:
                decisions_on = False
                set_mode(False)  # noqa: F821
        if band:
            _flash("#ffffff", "solid", WAVE_ACK_S,
                   then=_mic_on if decisions_on or memory_on else _idle)


def run(band_number):
    global band
    if band_number is None:
        print("TYPED input (no band, no microphone): one line per turn. /triple = triple tap, "
              "/wave = simulated three-wave memory toggle (typed context is not sent to the copilot).")
        while True:
            line = sys.stdin.readline(MAX_TEXT_BYTES + 1)
            if not line:
                break
            text = line.rstrip("\r\n")
            if len(text.encode("utf-8")) > MAX_TEXT_BYTES:
                while line and not line.endswith("\n"):
                    line = sys.stdin.readline(MAX_TEXT_BYTES + 1)
                publish(json.dumps({"type": "gap", "message": "Typed line exceeded the transcript byte limit and was rejected."}))  # noqa: F821
                continue
            command = text.strip()
            if command == "/triple":
                toggle_decisions()
            elif command == "/wave":
                on_three_waves()
            elif command:
                if not decisions_on and not memory_on:
                    print("(listening is off: type /triple or /wave first)")
                _emit(text, True)
        return

    from vexo_open.sync import Band

    band = Band.connect(band_number, auto_reconnect=True)
    _watch_gestures(bool(os.environ.get("VEXO_GESTURE_DEBUG")))
    print("Connected to %s. Triple-tap the band to start decisions mode." % band.name)
    while True:
        band.wait(1)


def _watch_gestures(debug=False):
    """Register the real event handlers separately so they can be tested without Bluetooth."""
    taps, wave = TapCounter(), WaveDetector()
    waves = TapCounter(taps=THREE_WAVES, window_ms=THREE_WAVE_WINDOW_MS)  # same windowed count as taps
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
        if wave.feed(m.t, getattr(m, WAVE_AXIS)):
            if waves.feed(m.t):
                threading.Thread(target=on_three_waves, daemon=True).start()
            else:  # one or two waves never change capture
                print("Wave %d of %d (within %d s)." % (len(waves.seen), THREE_WAVES, THREE_WAVE_WINDOW_MS // 1000))

    band.on("audio", lambda a: ears and ears.feed(a))
    band.on("tap", lambda e: on_tap(e, 1))
    band.on("double-tap", lambda e: on_tap(e, DOUBLE_TAP_COUNTS))
    band.on("motion", on_motion)
    band.set_events(kinds=["tap", "double-tap"], tap_threshold=TAP_THRESHOLD_G)
    band.on("reconnecting", lambda _: _status("off", "reconnecting"))
    band.on("reconnected", lambda _: _mic_on() if decisions_on or memory_on else _status("off", "connected"))
    band.on("disconnect", lambda _: _status("off", "off"))
    band.configure(events=True, motion=50)  # mic stays off until a triple tap or three waves
    _status("off", "connected")


def _mic_on():
    if band and (decisions_on or memory_on):
        band.configure(audio=True)  # solid blue: the microphone is on
        _status("live", "connected")


def _blink(hex_colour, then):
    if band:
        _flash(hex_colour, "blink", MODE_BLINKS * BLINK_PERIOD_MS / 1000, then=then)
    else:
        print("TYPED light (no band): %s blinks %d times" % (hex_colour, MODE_BLINKS))


def set_cue(hex_colour, pattern, validate=lambda: True):
    """Mic off (the band refuses LED commands otherwise), light for CUE_HOLD_S, mic back on."""
    if not _mode_lock.acquire(blocking=False):
        return False
    try:
        if not decisions_on or memory_on:
            return False
        if band:
            band.configure(audio=False, mic_level=False)
        # Flush first, then validate at the actual LED operation. The same locks protect
        # speech delivery, mode changes and LED commands, including blocking band calls.
        with _speech_lock:
            if ears:
                ears._sentence_ends()
            if not validate():
                _mic_on()
                return False
            if band is None:
                print("TYPED cue (no band):", hex_colour or "off", pattern)
            else:
                _flash(hex_colour, pattern, CUE_HOLD_S, then=_mic_on, mic_paused=True)
            return True
    finally:
        _mode_lock.release()


def _flash(hex_colour, pattern, seconds, then, mic_paused=False):
    """Mic off, the light for `seconds`, then `then()` (mic back on, or light off)."""
    global _resume
    if _resume:
        _resume.cancel()
    _resume = None
    if not mic_paused:
        band.configure(audio=False, mic_level=False)
    _status("cue-hold")
    try:
        if hex_colour:
            band.set_led(hex_colour, pattern=pattern, period=BLINK_PERIOD_MS if pattern == "blink" else 600)
        else:
            band.led_off()
    except Exception:
        # A failed LED command must not leave speech paused forever or report success.
        if decisions_on or memory_on:
            _mic_on()
        else:
            _status("off")
        raise
    def finish():
        with _mode_lock:
            if _resume is timer and then:
                then()

    timer = threading.Timer(seconds, finish)
    _resume = timer
    _resume.daemon = True
    _resume.start()
