#!/usr/bin/env python3
"""Fedo's wake-word listener ("Hey Fedo"), run by the daemon only when enabled.

Vosk's small English model in grammar-restricted mode: it can only say "hey
fido" (how the model spells Fedo — "fedo" isn't in its vocabulary) or [unk].
Silence is never decoded, which is what keeps the CPU near zero while nobody
talks. When the phrase is heard, the utterance's audio (16 kHz s16le mono) is
written to a file and its path is printed on stdout, one line:

    WAKE <path>

The daemon checks it again with whisper (so a false trigger costs one short
transcription and is dropped) and takes whatever was said after the phrase as
the command.

    wake.py --model <vosk model dir> [--wav file.wav]   (--wav: test, no mic)
"""
import array
import collections
import ctypes
import signal
import json
import math
import os
import subprocess
import sys
import tempfile
import wave

from vosk import KaldiRecognizer, Model, SetLogLevel

RATE = 16000
FRAME = 320                        # 20 ms
GRAMMAR = json.dumps(["hey fido", "hey", "fido", "[unk]"])
MAX_UTTERANCE = RATE * 15
DEBUG = bool(os.environ.get("FEDO_WAKE_DEBUG"))


def _die_with_parent():
    # PR_SET_PDEATHSIG = 1: the recorder never outlives this script.
    ctypes.CDLL("libc.so.6").prctl(1, signal.SIGTERM)


def frames_from_mic():
    p = subprocess.Popen(
        ["pw-record", "--rate", str(RATE), "--channels", "1", "--format", "s16", "--latency", "20ms", "-"],
        stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, preexec_fn=_die_with_parent)
    try:
        while True:
            b = p.stdout.read(FRAME * 2)
            if len(b) < FRAME * 2:
                return
            yield b
    finally:
        p.kill()


def frames_from_wav(path):
    with wave.open(path) as w:
        assert w.getframerate() == RATE and w.getnchannels() == 1
        data = w.readframes(w.getnframes())
    # Room tone before (it calibrates the noise floor) and a second of quiet
    # after (it ends the last utterance).
    data = b"\x00" * (RATE * 2 // 2) + data + b"\x00" * (RATE * 2)
    for i in range(0, len(data) - FRAME * 2 + 1, FRAME * 2):
        yield data[i:i + FRAME * 2]


def main():
    args = sys.argv[1:]
    model_dir = args[args.index("--model") + 1]
    wav = args[args.index("--wav") + 1] if "--wav" in args else None
    SetLogLevel(-1)
    rec = KaldiRecognizer(Model(model_dir), RATE, GRAMMAR)
    source = frames_from_wav(wav) if wav else frames_from_mic()

    # The first half second is the room, not the user: it sets the noise floor.
    # A room is rarely below the quiet minimum (a laptop mic sits near 0.015).
    floor = None
    calibration = []
    preroll = collections.deque(maxlen=25)      # 0.5 s before the speech
    utterance = []
    speaking = False
    quiet = 0
    for chunk in source:
        a = array.array("h", chunk)
        level = math.sqrt(sum(x * x for x in a) / len(a)) / 32768.0
        if floor is None:
            calibration.append(level)
            preroll.append(chunk)
            if len(calibration) >= 25:
                floor = sorted(calibration)[len(calibration) // 2]
            continue
        loud = level > max(floor * 2.5, 0.02)
        if not speaking:
            if not loud:
                floor = 0.995 * floor + 0.005 * level
                preroll.append(chunk)
                continue
            speaking = True
            quiet = 0
            utterance = list(preroll)
            preroll.clear()
        utterance.append(chunk)
        quiet = 0 if loud else quiet + 1
        done = rec.AcceptWaveform(chunk)
        if DEBUG and len(utterance) % 25 == 0:
            print(f"dbg frames={len(utterance)} level={level:.3f} floor={floor:.3f} partial={rec.PartialResult()}",
                  file=sys.stderr, flush=True)
        if quiet >= 30 or done or len(utterance) * FRAME > MAX_UTTERANCE:
            text = json.loads(rec.FinalResult() if not done else rec.Result()).get("text", "")
            rec.Reset()
            if DEBUG:
                print(f"dbg END text={text!r} frames={len(utterance)} quiet={quiet} done={done}", file=sys.stderr, flush=True)
            if "fido" in text.split() or text.startswith("hey fido"):
                fd, path = tempfile.mkstemp(prefix="fedo-wake-", suffix=".raw",
                                            dir=os.environ.get("XDG_RUNTIME_DIR"))
                with os.fdopen(fd, "wb") as f:
                    f.write(b"".join(utterance))
                print("WAKE " + path, flush=True)
            # Something that never went quiet is the room getting louder.
            if len(utterance) * FRAME > MAX_UTTERANCE:
                floor = max(floor, level * 0.8)
            speaking = False
            utterance = []
            quiet = 0


if __name__ == "__main__":
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(0))
    try:
        main()
    except BrokenPipeError:
        pass
