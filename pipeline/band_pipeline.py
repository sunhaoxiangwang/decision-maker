"""Python half of the bridge. Runs inside the Rust process (src/bridge.rs).

Rust sets `emit(text, speaker, is_final)` on this module, then calls run() on its own
thread and set_cue() whenever the backend has a decision. Only this process connects to
the band (one Bluetooth connection, AGENTS.md rule 7).

Live mode: one Deepgram WebSocket stays open and the band's audio streams into it with no
taps. A turn ends on Deepgram's UtteranceEnd: a gap after the last recognised *word*,
worked out from the transcript's word timings, so background noise cannot hold a turn
open the way it holds open the audio silence detector (speech_final, ignored here).
"""
import sys
import threading
from pathlib import Path

sys.stdout.reconfigure(line_buffering=True)  # embedded Python: print lines as they come
KIT = Path(__file__).resolve().parent.parent / "vexo-open-kit"
sys.path.insert(0, str(KIT / "examples" / "python"))  # speech.py (finds the library itself)

UTTERANCE_END_MS = 1000  # gap after the last word that ends a turn (Deepgram's minimum is 1000)
CUE_HOLD_S = 2.0      # the light only shows with the mic off; speech in this window is not heard

band = None
ears = None
_resume = None

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


def run(band_number):
    global band, ears
    if band_number is None:
        print("TYPED input (no band, no microphone): one line per turn.")
        for line in sys.stdin:
            if line.strip():
                emit(line.strip(), None, True)  # noqa: F821  (set by Rust)
        return

    from vexo_open.sync import Band

    band = Band.connect(band_number, auto_reconnect=True)
    ears = Diarized()
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
    if ears:
        ears._sentence_ends()  # words heard so far become a turn; with no sound, no UtteranceEnd would come
    band.configure(audio=False, mic_level=False)
    if hex_colour:
        band.set_led(hex_colour, pattern=pattern, period=600)
    else:
        band.led_off()
    _resume = threading.Timer(CUE_HOLD_S, lambda: band.configure(audio=True))
    _resume.daemon = True
    _resume.start()
