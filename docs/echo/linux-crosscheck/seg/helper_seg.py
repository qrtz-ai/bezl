"""Cycle 41 (ADR 0127, N168): a line-by-line port of the helper's PauseSegmenter (QrtzHelperCore/Segmented.swift,
SegmentRule defaults) for the Linux evals. seg_eval.py's own `segment()` is an earlier sketch of the rule (VAD floor
0.001 instead of vadMinRms 0.0002, no minSpeechMs, the cap not counting the pad, an unsmoothed quietest frame), close
at a normal level but not at a quiet one, where the floor decides what is speech. `seg_eval.py run --vad helper`
uses this. Float32 in Swift, float64 here: frame RMS agree to ~1e-7, so a cut can move only on an exact tie.
usage: python helper_seg.py   (selfcheck: the PauseSegmenter cases of SegmentedTests.swift)"""
import numpy as np

FRAME = 320  # 20 ms at 16 kHz


class Rule:
    def __init__(self, pauseMs=300, capMs=15_000, capSearchMs=1_500, segmentPadMs=640, minSpeechMs=200,
                 vadMinRms=0.0002, vadMarginDb=8.0, vadSpeechRelDb=-24.0, minSegmentMs=2_000):
        self.__dict__.update(locals()); del self.__dict__["self"]


class PauseSegmenter:
    def __init__(self, rule=None):
        self.rule = rule or Rule(); self.segmentStart = 0; self.position = 0; self.speechFrames = 0
        self.silenceRun = 0; self.floor = 0.001; self.speechLevel = 0.02; self.partial = []
        self.frameRms = []; self.frameSpeech = []

    def feed(self, s):
        """-> [(at, byCap, hadSpeech)] for the cuts these samples caused."""
        cuts = []; s = np.asarray(s, np.float32); i = 0
        while i < len(s):
            take = min(FRAME - len(self.partial), len(s) - i)
            self.partial.extend(s[i:i + take].tolist()); i += take
            if len(self.partial) == FRAME:
                c = self.frame_done(np.array(self.partial, np.float64))
                if c: cuts.append(c)
                self.partial = []
        return cuts

    def open_has_speech(self):
        return self.speechFrames > 0 or (len(self.partial) > 0 and self.is_speech(self.rms(np.array(self.partial))))

    @staticmethod
    def rms(f):
        return float(np.sqrt(np.sum(np.asarray(f, np.float64) ** 2) / max(1, len(f))))

    def is_speech(self, r):
        r_ = self.rule; margin = 10 ** (r_.vadMarginDb / 20); rel = 10 ** (r_.vadSpeechRelDb / 20)
        return r >= max(r_.vadMinRms, self.floor * margin, self.speechLevel * rel)

    def frame_done(self, f):
        r = self.rms(f); ru = self.rule
        self.floor = 0.9 * self.floor + 0.1 * r if r < self.floor else min(self.floor * 1.002, self.speechLevel * 0.1)
        speech = self.is_speech(r); self.position += len(f)
        self.frameRms.append(r); self.frameSpeech.append(speech)
        if speech: self.silenceRun = 0; self.speechFrames += 1; self.speechLevel = 0.95 * self.speechLevel + 0.05 * r
        else: self.silenceRun += 1
        ms = 20
        if (ru.pauseMs is not None and self.speechFrames * ms >= ru.minSpeechMs and self.silenceRun * ms >= ru.pauseMs
                and (self.position - (self.silenceRun // 2) * FRAME - self.segmentStart) // 16 >= ru.minSegmentMs):
            return self.cut(self.position - (self.silenceRun // 2) * FRAME, False)
        if ru.capMs is not None and (self.position - self.segmentStart) // 16 >= max(1_000, ru.capMs - ru.segmentPadMs):
            n = len(self.frameRms); look = max(1, min(n - 1, ru.capSearchMs // ms))
            def smooth(k):
                lo, hi = max(0, k - 2), min(n - 1, k + 2)
                return sum(self.frameRms[lo:hi + 1]) / (hi - lo + 1)
            best, bestE = n - 1, smooth(n - 1)
            for k in range(n - 1, n - look - 1, -1):
                e = smooth(k)
                if e < bestE: best, bestE = k, e
            return self.cut(self.segmentStart + best * FRAME, True)
        return None

    def cut(self, at, byCap):
        keep = (at - self.segmentStart) // FRAME
        had = any(self.frameSpeech[:keep])
        self.frameRms = self.frameRms[keep:]; self.frameSpeech = self.frameSpeech[keep:]
        self.speechFrames = sum(self.frameSpeech)
        run = 0
        for sp in reversed(self.frameSpeech):
            if sp: break
            run += 1
        self.silenceRun = run; self.segmentStart = at
        return (at, byCap, had)


def segment_helper(x, rule=None):
    """-> [(start, end, kind, hasSpeech)], kind 'pause' | 'cap' | 'end', as SegmentedTranscriber cuts a dictation
    fed whole (the segmenter is frame-based, so the block size doesn't change the cuts)."""
    s = PauseSegmenter(rule); out = []; s0 = 0
    for at, cap, had in s.feed(x):
        out.append((s0, at, "cap" if cap else "pause", had)); s0 = at
    out.append((s0, len(x), "end", s.open_has_speech()))
    return [c for c in out if c[1] > c[0]]


def _burst(n, wordMs=200, gapMs=100, pauses=None, first=1):
    a = []
    for k in range(n):
        a += [0.01 * (first + k)] * (16 * wordMs); a += [0.0] * (16 * (pauses or {}).get(k, gapMs))
    return np.array(a, np.float32)


def selfcheck():
    short = Rule(minSegmentMs=0)
    assert PauseSegmenter(short).feed(_burst(3, gapMs=200)) == [], "200 ms gaps are not pauses"
    a = np.concatenate([np.full(16 * 1000, 0.05, np.float32), np.zeros(16 * 400, np.float32), np.full(16 * 500, 0.05, np.float32)])
    assert PauseSegmenter(short).feed(a) == [(16 * 1160, False, True)]
    a = np.full(16 * 6000, 0.05, np.float32); a[16 * 4200:16 * 4300] = 0.02; a[16 * 4600:16 * 4620] = 0.01
    assert PauseSegmenter(Rule(capMs=5_000, minSegmentMs=0)).feed(a)[0] == (16 * 4240, True, True), "the middle of the gap"
    c = PauseSegmenter(Rule(minSegmentMs=2_000)).feed(np.concatenate([_burst(8, pauses={3: 400, 7: 400}), _burst(2, first=9)]))
    assert len(c) == 1 and c[0][0] >= 16 * 2000
    # Block size doesn't move a cut.
    s = PauseSegmenter(short); got = []
    for i in range(0, len(a := np.concatenate([_burst(9, pauses={2: 500, 5: 600})])), 1360): got += s.feed(a[i:i + 1360])
    assert got == PauseSegmenter(short).feed(a) and len(got) == 2
    # Whole clip: no cuts.
    assert segment_helper(a, Rule(pauseMs=None, capMs=None)) == [(0, len(a), "end", True)]
    print("helper_seg selfcheck ok")


if __name__ == "__main__":
    selfcheck()
