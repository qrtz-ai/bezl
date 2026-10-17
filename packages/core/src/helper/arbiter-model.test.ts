/**
 * Independent reference model for the helper arbiter (H5/H6), checked exhaustively over every reachable
 * (arbiter state, model state) pair x every event, which covers all event sequences of any length, plus seeded
 * random length-8 replays. The model tracks MicHub refcounts by replaying the arbiter's own effect strings, so a
 * double release or a leaked subscription shows up as a negative/orphaned ref. Also runs the edge vectors in
 * spec/vectors/helper/arbiter-edges.json (reviewer-added; the Swift side should consume them too).
 */
import fs from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";
import {
  arbiterStep,
  initialArbiterState,
  type HelperArbiterEvent,
  type HelperArbiterState,
  type HelperPermission,
} from "./arbiter";

const PERMS: HelperPermission[] = [
  "microphone",
  "screen-recording",
  "accessibility",
];
const EVENTS: HelperArbiterEvent[] = [
  { cmd: "recording.start" },
  { cmd: "recording.stop" },
  { cmd: "dictation.start" },
  { cmd: "dictation.stop" },
  { cmd: "dictation.cancel" },
  ...PERMS.map((p) => ({ revoked: p })),
  ...PERMS.map((p) => ({ granted: p })),
  { helperLost: true as const },
];

/** What the model believes, built only from the spec text, never from arbiter.ts. */
interface Model {
  recording: boolean;
  dictating: boolean;
  /** Recording holds a mic ref: started with mic usable and no mic revoke since. */
  recMic: boolean;
  denied: Set<HelperPermission>;
  /** MicHub refcounts reconstructed from emitted effects. */
  refs: Record<string, number>;
}

const initModel = (denied: HelperPermission[]): Model => ({
  recording: false,
  dictating: false,
  recMic: false,
  denied: new Set(denied),
  refs: {},
});

function expectedDegrade(
  p: HelperPermission,
  denied: Set<HelperPermission>,
): string {
  if (p === "accessibility") return "ax-unavailable";
  const other = p === "microphone" ? "screen-recording" : "microphone";
  if (denied.has(other)) return "audio-unavailable";
  return p === "microphone" ? "no-mic" : "mic-only";
}

/** Spec transition. Returns expected decision and the expected effect set. */
function modelStep(
  m: Model,
  e: HelperArbiterEvent,
): { m: Model; decision: string; effects: string[] } {
  const n: Model = { ...m, denied: new Set(m.denied), refs: { ...m.refs } };
  const fx: string[] = [];
  if ("helperLost" in e) {
    if (m.dictating) fx.push("dictation.stop:helper-lost");
    // Process is gone: the hub dies with it, so every ref is released implicitly.
    return {
      m: { ...initModel([...m.denied]) },
      decision: "accept",
      effects: fx,
    };
  }
  if ("granted" in e) {
    n.denied.delete(e.granted); // never resubscribes a running recording, never clears degradations
    return { m: n, decision: "accept", effects: [] };
  }
  if ("revoked" in e) {
    if (m.denied.has(e.revoked)) return { m, decision: "ignore", effects: [] };
    n.denied.add(e.revoked);
    if (e.revoked === "microphone") {
      if (m.dictating) {
        fx.push(
          "dictation.stop:permission-revoked",
          "mic.unsubscribe:dictation",
        );
        n.dictating = false;
      }
      if (m.recMic) {
        fx.push("mic.unsubscribe:recording");
        n.recMic = false;
      }
    }
    if (m.recording) fx.push(`degrade:${expectedDegrade(e.revoked, m.denied)}`);
    return { m: n, decision: "accept", effects: fx };
  }
  switch (e.cmd) {
    case "recording.start":
      if (m.recording) return { m, decision: "ignore", effects: [] };
      n.recording = true;
      n.recMic = !m.denied.has("microphone");
      if (n.recMic) fx.push("mic.subscribe:recording");
      return { m: n, decision: "accept", effects: fx };
    case "recording.stop":
      if (!m.recording) return { m, decision: "ignore", effects: [] };
      if (m.recMic) fx.push("mic.unsubscribe:recording");
      n.recording = false;
      n.recMic = false;
      return { m: n, decision: "accept", effects: fx };
    case "dictation.start":
      if (m.dictating) return { m, decision: "ignore", effects: [] };
      if (m.denied.has("microphone"))
        return { m, decision: "refuse", effects: [] };
      n.dictating = true;
      return { m: n, decision: "accept", effects: ["mic.subscribe:dictation"] };
    default:
      if (!m.dictating) return { m, decision: "ignore", effects: [] };
      n.dictating = false;
      return {
        m: n,
        decision: "accept",
        effects: ["mic.unsubscribe:dictation"],
      };
  }
}

/** Apply effects to the refcount, failing on double subscribe / double release. */
function applyRefs(
  refs: Record<string, number>,
  effects: string[],
  ctx: string,
) {
  for (const f of effects) {
    const [k, who] = f.split(":");
    if (k === "mic.subscribe") {
      expect(refs[who!] ?? 0, `double subscribe ${who} @ ${ctx}`).toBe(0);
      refs[who!] = 1;
    } else if (k === "mic.unsubscribe") {
      expect(refs[who!] ?? 0, `release without ref ${who} @ ${ctx}`).toBe(1);
      refs[who!] = 0;
    }
  }
}

function checkStep(
  s: HelperArbiterState,
  m: Model,
  e: HelperArbiterEvent,
  ctx: string,
) {
  const r = arbiterStep(s, e);
  const x = modelStep(m, e);
  expect(r.decision, `decision @ ${ctx}`).toBe(x.decision);
  expect([...r.effects].sort(), `effects @ ${ctx}`).toEqual(
    [...x.effects].sort(),
  );
  if (r.decision !== "accept") {
    expect(r.state, `non-accept must not change state @ ${ctx}`).toEqual(s);
    expect(r.effects).toEqual([]);
    expect(r.reason, `ignored/refused needs a reason @ ${ctx}`).toBeTruthy();
  }
  const refs = { ...m.refs };
  if ("helperLost" in e) for (const k of Object.keys(refs)) refs[k] = 0;
  else applyRefs(refs, r.effects, ctx);
  x.m.refs = refs;
  // Invariants on the arbiter's state.
  const st = r.state;
  expect(st.recording).toBe(x.m.recording);
  expect(st.dictating).toBe(x.m.dictating);
  expect(new Set(st.denied)).toEqual(x.m.denied);
  expect(new Set(st.mic).size, `dup mic subscriber @ ${ctx}`).toBe(
    st.mic.length,
  );
  expect(new Set(st.mic)).toEqual(
    new Set([
      ...(x.m.recMic ? ["recording"] : []),
      ...(x.m.dictating ? ["dictation"] : []),
    ]),
  );
  // Effect-replayed refcount must equal the arbiter's subscriber list (no leak, no orphan).
  expect(new Set(Object.keys(refs).filter((k) => refs[k] === 1))).toEqual(
    new Set(st.mic),
  );
  // Confinement: nobody reads the mic while it is denied.
  if (st.denied.includes("microphone"))
    expect(st.mic, `mic read while denied @ ${ctx}`).toEqual([]);
  expect(r.mux).toBe(
    st.mic.includes("recording") && st.mic.includes("dictation"),
  );
  return { r, m: x.m };
}

const key = (s: HelperArbiterState, m: Model) =>
  JSON.stringify([s, [...m.denied].sort(), m.recMic, m.refs]);
const subsets = <T>(xs: T[]): T[][] =>
  xs.reduce<T[][]>((a, x) => [...a, ...a.map((y) => [...y, x])], [[]]);

describe("helper arbiter: independent reference model", () => {
  it("agrees with the spec model on every reachable state x event (all sequence lengths)", () => {
    let explored = 0;
    for (const init of subsets(PERMS)) {
      const seen = new Set<string>();
      const q: [HelperArbiterState, Model, string][] = [
        [
          initialArbiterState([...init]),
          initModel(init),
          `init${JSON.stringify(init)}`,
        ],
      ];
      while (q.length) {
        const [s, m, trace] = q.shift()!;
        const k = key(s, m);
        if (seen.has(k)) continue;
        seen.add(k);
        for (const e of EVENTS) {
          const ctx = `${trace} -> ${JSON.stringify(e)}`;
          const { r, m: m2 } = checkStep(s, m, e, ctx);
          explored++;
          q.push([r.state, m2, ctx]);
        }
      }
    }
    expect(explored).toBeGreaterThan(1000);
  });

  it("seeded random length-8 sequences hold the same properties", () => {
    let seed = 0x5eed;
    const rnd = (n: number) =>
      (seed = (seed * 1103515245 + 12345) & 0x7fffffff) % n;
    for (let i = 0; i < 3000; i++) {
      const init = subsets(PERMS)[rnd(8)]!;
      let s = initialArbiterState([...init]);
      let m = initModel(init);
      const trace: string[] = [];
      for (let j = 0; j < 8; j++) {
        const e = EVENTS[rnd(EVENTS.length)]!;
        trace.push(JSON.stringify(e));
        const out = checkStep(
          s,
          m,
          e,
          `${JSON.stringify(init)} ${trace.join(" ")}`,
        );
        s = out.r.state;
        m = out.m;
      }
    }
  });
});

const edges = JSON.parse(
  fs.readFileSync(
    path.resolve(
      __dirname,
      "../../../../spec/vectors/helper/arbiter-edges.json",
    ),
    "utf8",
  ),
) as {
  cases: {
    name: string;
    initialDenied: HelperPermission[];
    steps: { event: HelperArbiterEvent; expect: Record<string, unknown> }[];
  }[];
};

describe("helper arbiter: edge vectors (spec/vectors/helper/arbiter-edges.json)", () => {
  for (const c of edges.cases)
    it(c.name, () => {
      let s = initialArbiterState(c.initialDenied);
      let m = initModel(c.initialDenied);
      for (const [i, st] of c.steps.entries()) {
        const out = checkStep(s, m, st.event, `${c.name} step ${i}`);
        expect({ ...out.r, reason: out.r.reason }).toEqual({
          reason: undefined,
          ...st.expect,
        });
        s = out.r.state;
        m = out.m;
      }
    });
});
