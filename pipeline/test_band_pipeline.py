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
