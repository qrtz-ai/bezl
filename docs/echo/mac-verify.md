# echo: Mac verification for echo cycle 73 (ADR 0222, ADR 0224)

Bozniak's plan. None of this can run on the Linux box; run it on Jared's Mac (M1 Pro) with a build from main.
Record results under each step (pass/fail, notes, numbers) and send them to Bozniak.

## 1. Mic pull and re-press (N194, N195)

1. A USB mic (or AirPods) as input. Hold fn, speak, release. It inserts.
2. Hold fn, speak, and pull the mic (or take the AirPods out) at the release. Press fn again at once while the
   last dictation finalizes. Expect: the last dictation's words are inserted and in history; no "microphone
   access denied".
3. With the mic still out (built-in mic disabled, or no other input), press fn. Expect: "no microphone was found,
   so dictation didn't start" (mic-lost), not "microphone access denied".
4. Plug the mic back in. Press fn and speak. Expect: it hears and inserts on the first press (before N195 every
   press heard nothing until the warm setting was toggled).
5. Repeat 2 to 4 with the warm mic on and off.

## 2. Lid close while holding (N196)

1. Hold fn, start speaking, close the lid while still holding, let go, wait 10 s, open the lid.
2. Expect: the dictation ended at sleep (inserted or kept in history), the pill is not stuck listening, and the
   next fn press works.
3. Variant: hold fn through the close and keep holding through the wake. Expect: the dictation ended at sleep;
   the held key does not start a new dictation until it is pressed again.

## 3. Latency, 20 runs

20 dictations each on the build before this change and on this build, same phrase and app: key release to text
inserted (the pill's timing log, or `echo-bench-timer`). Report median and p90 for both. Expect no change (the
change adds one mic-state read after each admit).

## 4. echo-bench --quick WER

`echo-bench --quick` on both builds. Report WER for both. Expect no change.
