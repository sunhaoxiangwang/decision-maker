"""Python half of the bridge. Runs inside the Rust process (src/bridge.rs).

Rust sets `emit(text, speaker, is_final)` on this module, then calls run() on its own
thread and set_cue() whenever the backend has a decision. Only this process connects to
the band (one Bluetooth connection, AGENTS.md rule 7).

Live mode: one Deepgram WebSocket stays open and the band's audio streams into it with no
taps. Every pause Deepgram hears ends a turn (final=True).
"""
import sys
import threading
from pathlib import Path

sys.stdout.reconfigure(line_buffering=True)  # embedded Python: print lines as they come
KIT = Path(__file__).resolve().parent.parent / "vexo-open-kit"
sys.path.insert(0, str(KIT / "examples" / "python"))  # speech.py (finds the library itself)

ENDPOINTING_MS = 500  # pause that ends a turn; the kit's 100 ms splits sentences mid-thought
CUE_HOLD_S = 2.0      # the light only shows with the mic off; speech in this window is not heard

band = None
_resume = None


def run(band_number):
    global band
    if band_number is None:
        print("TYPED input (no band, no microphone): one line per turn.")
        for line in sys.stdin:
            if line.strip():
                emit(line.strip(), None, True)  # noqa: F821  (set by Rust)
        return

    import speech
    from vexo_open.sync import Band

    class Diarized(speech.DeepgramSpeech):
        """The kit's Deepgram engine, plus the speaker of the latest words (diarize=true)."""
        speaker = None

        def _message(self, message):
            alts = (message.get("channel") or {}).get("alternatives") or [{}]
            found = [w["speaker"] for w in alts[0].get("words") or [] if "speaker" in w]
            if found:
                self.speaker = max(set(found), key=found.count)
            super()._message(message)

    band = Band.connect(band_number, auto_reconnect=True)
    ears = Diarized(endpointing=ENDPOINTING_MS, diarize="true")
    ears.start(lambda text, final: emit(text, ears.speaker, final))  # noqa: F821
    band.on("audio", ears.feed)
    band.configure(audio=True)  # solid blue: the microphone is on
    print("Listening live on %s. Tell the people nearby." % band.name)
    while True:
        band.wait(1)


def set_cue(hex_colour, pattern):
    """Mic off (the band refuses LED commands otherwise), light for CUE_HOLD_S, mic back on."""
    global _resume
    if band is None:
        print("TYPED cue (no band):", hex_colour or "off", pattern)
        return
    if _resume:
        _resume.cancel()
    band.configure(audio=False, mic_level=False)
    if hex_colour:
        band.set_led(hex_colour, pattern=pattern, period=600)
    else:
        band.led_off()
    _resume = threading.Timer(CUE_HOLD_S, lambda: band.configure(audio=True))
    _resume.daemon = True
    _resume.start()
