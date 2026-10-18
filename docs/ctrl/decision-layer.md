# Ctrl decision layer: spec (draft, 2026-10-17)

Status: draft from Jared's design. Research basis: local-decision-models.md (same folder). No code yet.

## 1. Principle

Ctrl doesn't send one big prompt with the whole screen. It runs a **chain of small decisions**, narrowing down step by step the way a person does. Each decision node:

- sees a small state slice: the goal, the current narrowed AX subtree, and the last few steps;
- chooses from a **small enumerated option set**, with a target of **20 or fewer options** and a hard cap of 64;
- returns a **typed answer with a calibrated probability**.

Small option sets keep each call to a short prompt read with no text generation. They also keep calibration meaningful, since the scorers are weak on large label sets (Laya: 0.425 on 72 labels). Every step stays inspectable and replayable.

## 2. Worked example: "open youtube"

1. **choose_app.** Options: installed apps ranked by prior. The default browser goes first, then other browsers, then the YouTube app or web-app if one is installed. The decision: Safari (p=0.93). Launch or activate it through NSWorkspace.
2. **choose_move** (shortcut or branch). The options mix keyboard shortcuts with the top-level AX branches of the front window:
   - shortcuts: `cmd-L focus address bar`, `cmd-T new tab`
   - branches: `toolbar`, `tab bar`, `web content`, `sidebar`
     The decision: `cmd-L` (p=0.88). Shortcuts are first-class options: they're cheap, deterministic, and need no tree walk.
3. **choose_text.** The candidates:
   - `youtube.com`
   - a history or bookmark hit for `https://www.youtube.com/`
   - `search: youtube`
     The decision: `youtube.com` (p=0.95). Type it, then press Return.
4. **verify.** Read the AX state: is the focused window's URL or title on youtube.com, and has the page finished loading? A yes/no check: p(done)=0.97. **Done.**

That's 4 small calls and no screenshot.

## 3. Node types

| Node                      | Input slice                                                                        | Option source                                                                                                                                                                                                                                                 | Output                                             |
| ------------------------- | ---------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------- |
| `choose_app`              | goal plus the app catalog                                                          | Installed apps from LaunchServices, plus running apps. Ranked by priors: the default handler for the URL scheme or file type, recency, and frequency. Top 20 are kept.                                                                                        | app id plus p                                      |
| `choose_move`             | goal plus the front window's top-level AX branches plus the shortcut table         | Shortcuts from (a) the app's menu bar items with key equivalents, read from AX, so they come free and are always accurate, and (b) a built-in table of common shortcuts. Branches are the direct AX children, each summarized as role, label and child count. | shortcut id, or the branch to descend into, plus p |
| `choose_element_action`   | goal plus the current subtree (one level, or a flattened leaf list of 20 or fewer) | (element, action) pairs: AXPress, AXSetValue, AXShowMenu, focus, scroll                                                                                                                                                                                       | pair plus p                                        |
| `choose_text`             | goal plus the focused field's role, label and placeholder                          | Generated candidate list: URL guesses, local history and bookmarks, a search query, file paths, contacts. If no candidate fits, fall back to **tightly constrained short generation** (grammar-limited, 32 tokens or fewer).                                  | candidate id or the constrained string, plus p     |
| `verify`                  | goal plus the post-action AX state diff                                            | yes/no ("goal reached?", "expected change happened?")                                                                                                                                                                                                         | p(done), p(error)                                  |
| `confirm_gate` (implicit) | the pending action                                                                 | yes/no "irreversible?" plus a static rules table                                                                                                                                                                                                              | proceed, or ask Jared                              |

The planner (§7) splits multi-step tasks into subgoals, and each subgoal runs this chain.

## 4. Keeping options small

- A **pruning pass** runs before scoring. It drops hidden, disabled and zero-size elements, and collapses AXGroup-only wrappers.
- If a level has more than 20 children: **group and summarize**. Cluster by role, row or column, and section heading. Present each cluster as one option ("list: 140 messages", "toolbar: 9 buttons"). Choosing a cluster descends into it, so the narrowing is recursive.
- Priors re-rank and truncate to the top 20: label text overlap with the goal, recency, and elements the user has acted on before.
- A deterministic "none of these" option is always included. Choosing it triggers a backtrack or the fallback.

## 5. Confidence, backtracking, asking

- Each node returns the top-1 probability `p`. It is calibrated per node type with temperature scaling, fitted on the replay set from §10.
- **p ≥ τ_act** (start at 0.80): act.
- **τ_back ≤ p < τ_act** (start at 0.50): act tentatively. Push the alternatives onto a backtrack stack. If `verify` fails, pop and try the next-best option, at most 2 backtracks per node and 6 per task.
- **p < τ_back, or the backtrack budget runs out**: **ask Jared**. Show the top 3 options in lowercase UI text ("which one: …").
- Thresholds are per node type and tuned from the replay set. They are not hard-coded forever.

## 6. Narrowed-tree cache

- Between steps, keep the narrowed path (app → window → branch → subtree), the element references, and the summaries.
- Invalidate on AX notifications: AXFocusedUIElementChanged, AXValueChanged, AXCreated / AXUIElementDestroyed, window change. Otherwise re-read only the subtree that changed.
- Also cache per app: the shortcut table (from the menu bar) and the cluster summaries of stable chrome such as toolbars and sidebars.
- The cache lives in memory only, is never written to disk, and is cleared when Ctrl goes idle. AX content is user data, and ADR 0083's logging rules apply: never log labels or values.

## 7. Models (all on-device)

| Role                                                              | Model                                                                                                                                                                                        | Why                                                                                            |
| ----------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------- |
| Node scorer, default                                              | **Small Qwen3 (1.7–4B, 4-bit) in MLX, using option scoring**: enumerate options with one-token labels, read one prompt pass, take a softmax over the label logits, apply temperature scaling | A full calibrated distribution in one call. Handles 20+ options and longer AX slices.          |
| Fast yes/no gates (`verify`, `confirm_gate`, simple `choose_app`) | **Laya on Core ML** (0.3–0.4B encoder, Apache-2.0)                                                                                                                                           | Millisecond-class, calibrated. 512–1,024 token limit, which fits pruned slices.                |
| Constrained text                                                  | the same Qwen3, with a grammar or token mask                                                                                                                                                 | Short and always valid                                                                         |
| Planner, multi-step tasks only                                    | **Apple Foundation Models** (tool calling, @Generable)                                                                                                                                       | No download needed. **It gives no probabilities, so it never gates and never scores options.** |
| Fallback locator                                                  | Screenshot GUI model (Holo2-30B-A3B, or Step-GUI-4B / Qwen3-VL-4B to save memory), downloaded with consent                                                                                   | Only for the cases in §8                                                                       |

## 8. Fallback for poor or flat trees

Triggers, any of:

- a level with more than 20 unlabeled children after grouping;
- "none of these" chosen twice;
- an app known to have a poor tree (Electron, canvas, games);
- the AX tree is empty.

The fallback goes in order:

1. **Group and summarize** by geometry and role (§4).
2. Use the **OCR-labeled tree**: Vision text recognition on the window, so unlabeled elements get labels from the text inside their frame.
3. Use the **screenshot locator**: the VLM returns a point or box for the subgoal, and Ctrl acts through CGEvent. The locator's output then goes through `verify` like any other action.

## 9. Confirm gate

Before any irreversible action, Ctrl asks Jared, and the request shows the exact action. Irreversible actions include:

- send, post, submit, purchase, delete, empty trash, overwrite, quit with unsaved changes;
- system settings changes;
- anything in a password or payment field.

The gate is a **static rules table** (role, label keywords, app) **OR** Laya's "irreversible?" score at 0.3 or above. It errs toward asking. The pattern is Bound→Confirm→Act. Jared can't turn the gate off for the irreversible classes.

## 10. Offline constraint

- No network calls at runtime. Model downloads happen only from onboarding or settings, with consent, never on first use. This follows ADR 0076.
- History and bookmark candidates come from local browser data only, and only with permission.
- `choose_text` never calls a search API. "search: X" means typing X into the app's own field.

## 11. Latency budget (targets to verify, not measured)

| Step                                      | Target p50             | Notes                                                                                                                                          |
| ----------------------------------------- | ---------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------- |
| AX read and prune (cached)                | ≤ 30 ms                |                                                                                                                                                |
| Laya gate                                 | ≤ 30 ms                | 33 ms per question on a T4 (sourced); Core ML on the Mac still to be measured                                                                  |
| Qwen3 option scoring (≤ 1k-token slice)   | ≤ 400 ms               | Prompt-reading speed is about 1.2k tok/s on an M3 Max with Qwen3-30B-A3B (sourced). Small dense models should be faster; still to be measured. |
| Action plus settle                        | ≤ 300 ms               | Until the AX notification arrives                                                                                                              |
| **Simple task ("open youtube", 4 nodes)** | **≤ 2.5 s end to end** |                                                                                                                                                |
| Planner call (multi-step only)            | ≤ 3 s                  | Apple FM is slow at generation, so it runs once per task, not per step                                                                         |

Model memory budget: the resident scorer plus Laya should stay at 3 GB or less. The screenshot locator is loaded on demand only.

## 12. Mac benchmark plan: hierarchical chain vs flat scoring

**Arms**

- **H, hierarchical:** this spec's node chain.
- **F, flat:** one option-scoring call per step over all actionable elements in the window plus shortcuts. Same Qwen3 model, same calibration.
- Optional **F-FM** arm: Apple FM with @Generable choosing the action from the flat list, with no probabilities.

**20 tasks** (fixed set, run 3 times each, on Jared's M-series Mac with macOS 27):

1. open youtube
2. open a new safari tab to apple.com
3. search notes for "groceries"
4. create a new note titled "test"
5. open system settings → wi-fi
6. toggle dark mode
7. open the downloads folder in finder
8. rename a file in finder
9. start a new message in messages (stop at the confirm gate)
10. set a 10-minute timer in clock
11. play or pause music
12. open the last-used document in pages
13. find "invoice" in mail search
14. add a calendar event tomorrow at 3 PM (stop at the confirm gate)
15. change the safari zoom to 125%
16. open terminal and cd to ~/code
17. switch to the second desktop
18. open qrtz echo settings
19. empty the trash (must hit the confirm gate)
20. an Electron app action, such as clicking a channel in Slack, to exercise the poor-tree fallback

**Metrics, per task and aggregated**

- success rate, checked by a scripted AX check
- confirm-gate correctness: there should be zero missed irreversible actions
- ask rate and backtracks
- end-to-end latency p50 and p95, plus per-node latency
- prompt tokens per step
- peak RSS memory and resident model memory
- calibration: ECE and Brier per node type

**Procedure**

- Warm the models first.
- Use clean fixture state for each task, with a reset script.
- Log only ids, timings and probabilities, with no AX text.
- Build the replay set from these runs and fit the thresholds on it.

**Decision rule:** adopt H if its success rate is at least F's and its p50 latency is lower. If F wins on short tasks, allow flat scoring for windows with 20 or fewer actionable elements.

## 13. Core: Redux-like deterministic state engine (from Jared)

### 13.1 Model

- There is **one state** (`CtrlState`) and **one append-only event log**. State is always `fold(reducers, initial, events[0..n])`.
- Reducers are **pure**. They make no I/O, no clock reads, no randomness and no model calls. Time, ids and seeds arrive inside events.
- **Side effects live only in effects (middleware).** Effects read state and events and do the outside-world work:
  - the AX observer
  - the model scorer
  - the OS action executor (AXPress / AXSetValue / CGEvent / NSWorkspace)
  - the confirm prompt
    Each effect reports its result as a new event. Effects never mutate state directly.
- Determinism contract: same log in, byte-identical state out. In debug builds, a strict replay check re-folds the log and compares state hashes.

### 13.2 Event schema

Every event carries a common envelope: `{seq, run_id, parent_seq?, t_mono_ns, t_wall, kind, schema_v, payload}`.

- `seq` is monotonic per run.
- `parent_seq` is set on the first event of a fork.

| kind                                     | payload                                                                                                                                                                                                                                                                                   |
| ---------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `task.started`                           | goal_ref (a redacted id plus a local pointer, see §13.5), planner?                                                                                                                                                                                                                        |
| `plan.produced`                          | subgoal list (ids plus the redacted text pointer), model plus version                                                                                                                                                                                                                     |
| `obs.snapshot`                           | full pruned AX tree for the app/window: node ids, roles, frames, label hashes, label_ref (pointer). Content-addressed by hash.                                                                                                                                                            |
| `obs.diff`                               | base snapshot hash, list of node add / remove / change ops                                                                                                                                                                                                                                |
| `decision.made`                          | node_type, option list (ids plus label_ref plus kind: shortcut / branch / element_action / text_candidate / app), scores and probabilities, temperature T, chosen id, thresholds, outcome (act / tentative / ask), model id plus version plus quantization, seed, prompt hash, latency_ms |
| `decision.answered_by_user`              | the option chosen in an ask, or "cancel"                                                                                                                                                                                                                                                  |
| `confirm.requested` / `confirm.resolved` | action ref, rule ids or gate score, approve or deny                                                                                                                                                                                                                                       |
| `action.requested`                       | typed action: key_combo, ax_press(node), ax_set_value(node, text_ref), click(point), launch(app)                                                                                                                                                                                          |
| `action.executed`                        | result (ok / err code), settle_ms                                                                                                                                                                                                                                                         |
| `verify.result`                          | p_done, p_error, pass or fail                                                                                                                                                                                                                                                             |
| `backtrack`                              | to_seq, reason                                                                                                                                                                                                                                                                            |
| `undo.requested` / `undo.executed`       | target action seq, inverse action, ok or not possible                                                                                                                                                                                                                                     |
| `task.finished`                          | status (done / failed / cancelled / asked_out)                                                                                                                                                                                                                                            |

### 13.3 Reducers (pure, by slice)

- `obs` reducer: applies a snapshot or diff to `state.screen`, giving the current AX tree and the narrowed-tree cache (§6).
- `chain` reducer: the decision stack, current node, backtrack stack and budgets.
- `task` reducer: the goal, subgoals, status and pending confirm.
- `history` reducer: undo inverses for reversible actions.

The selectors `currentOptions(state)` and `needsConfirm(state)` are pure functions.

### 13.4 Replay, time travel, fork, undo

- **Scrub or time-travel:** fold `events[0..k]`. Nothing executes and no model is called.
- **Exact replay:** effects run in _replay mode_.
  - The scorer effect returns the recorded `decision.made` instead of calling the model.
  - The executor is a no-op, or in "re-drive" mode it re-executes against the live OS and checks each `obs.diff` against the recorded one. A mismatch raises `replay.divergence` at the first differing seq.
- **Fork:** copy the prefix `[0..k]` into a new run with `parent_seq = k`, then continue live. That allows a different option, a different model or a changed threshold. Forks are first-class and can be diffed against the parent.
- **Undo, where possible:** each `action.requested` records its inverse where one exists:
  - `ax_set_value` → restore the previous value
  - toggles → toggle back
  - Finder moves and renames → move back
  - newly opened tab or window → close it
    Undo appends `undo.requested` / `undo.executed`. **The log is never rewritten.** Sends, purchases and other irreversible actions have no inverse, which is exactly why they need the confirm gate.

### 13.5 Snapshots vs diffs

- Write `obs.snapshot`:
  - on app or window switch;
  - every 50 events;
  - when the diff would be larger than 30% of the tree.
- Write `obs.diff` otherwise.
- Keep reducer-state checkpoints every 100 events, so scrubbing is O(100) folds and not O(n).
- Snapshot blobs are content-addressed by SHA-256 and deduplicated.

### 13.6 Storage, privacy, redaction (consistent with ADR 0083)

- **Local only:** `~/Library/Application Support/qrtz/ctrl/runs/<run_id>/`, holding the event log (append-only, length-prefixed records, hash chain over `seq`) plus a CAS blob store. Nothing syncs, nothing uploads, and there is no telemetry.
- **The log holds no content.** Text fields such as labels, typed text, goals and URLs never appear inline. They are stored as `*_ref` pointers into a separate **encrypted content store**, with the key held in the Keychain. Events carry hashes and pointers only. The ADR 0083 rule applies to the log too: no field may be named text, transcript, title, prompt or summary.
- **Never stored at all:**
  - secure text fields (AXSecureTextField)
  - password-manager windows
  - anything matched by the redaction rules (card numbers, one-time codes)
    These are logged as `redacted` with a reason code.
- **Retention:** 7 days by default, with a user-set value and a "delete all history" control. Deleting the content store leaves an id-and-timing skeleton that still replays structurally.
- The OS-level operational logs (`~/Library/Logs/qrtz`) stay separate and follow ADR 0083 unchanged.

### 13.7 History scrubber UI (@qrtz/ui, lowercase, Geist Pixel Circle, Feather icons)

- **Timeline strip:** one tick per event, colored by kind (obs / decision / action / confirm / verify). A play / pause / step control steps one event at a time.
- **Decision inspector:** at a selected tick, shows the node type, the options with probability bars, the chosen option, thresholds, the model and version, and latency.
- **Screen view:** the AX tree at that point, with the acted-on element highlighted. A screenshot appears only if the fallback locator captured one, and only while the content store still holds it.
- **Actions:**
  - "fork from here", which opens a new run seeded at that point
  - "undo to here", which offers only the reversible actions after this point and lists the irreversible ones
  - "replay" (dry) and "re-drive" (live, with a divergence check)
  - "export run", which is local only and redacted
- Forks appear as branches on the timeline.

### 13.8 Prior art: which computer-use agent was built like this?

I couldn't confirm a well-known _desktop_ computer-use agent that is explicitly "Redux-style" (pure reducers over a single store). The closest real matches:

| Project                               | What matches                                                                                                                                                                                                                                                                                                         | Borrow                                                                              | Link                                                                                                                                                                                                                                                  |
| ------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **OpenHands** (V1 Software Agent SDK) | **Best known agent built on an immutable, append-only event log.** Actions and observations are events, the log is the agent's memory, and the event store supports parent ids and branches. Legacy trajectory replay re-issued recorded actions. It's a coding agent rather than a GUI agent.                       | Event taxonomy (action vs observation), parent-id branching, the event store design | https://docs.openhands.dev/sdk/arch/events, https://github.com/OpenHands/software-agent-sdk/blob/main/openhands-sdk/openhands/sdk/conversation/event_store.py, https://github.com/All-Hands-AI/OpenHands/blob/3ec999e8/openhands/controller/replay.py |
| **Loom** (mentiora-ai)                | **Closest in spirit for a computer or browser agent:** a "deterministic action store", a write-ahead log with a SHA-256 hash chain, `session replay`, `inspect --at-action N` time travel with no re-run, content-addressed DOM and screenshot artifacts. Written in Rust and uses the macOS Keychain. Browser only. | Hash-chained log, CAS artifacts, time-travel inspect, typed errors                  | https://github.com/mentiora-ai/loom                                                                                                                                                                                                                   |
| **Stately Agent** (XState)            | Agent runtime whose event log is "the source of truth". `replay` is a pure fold with no model calls; `forkEventLog(entries, n)`.                                                                                                                                                                                     | Exact replay / fork semantics, conformance tests                                    | https://stately.ai/docs/packages/agent/event-log                                                                                                                                                                                                      |
| Active Graph                          | Fork at an event, prompt-hash cache, strict-replay divergence errors                                                                                                                                                                                                                                                 | Strict vs permissive replay, `replay-divergence-error`                              | https://docs.activegraph.ai/concepts/forking/, https://docs.activegraph.ai/concepts/replay/                                                                                                                                                           |
| AgentReplay                           | Event-sourced browser-agent log (DomSnapshotCaptured, AgentActionExecuted, DivergenceDetected)                                                                                                                                                                                                                       | Event names, divergence event                                                       | https://github.com/Zwc-11/agentreplay/blob/main/docs/architecture.md                                                                                                                                                                                  |
| LangGraph time travel                 | Checkpoint per step, replay and fork through `update_state`. **But replay re-executes LLM calls.**                                                                                                                                                                                                                   | Checkpoint plus fork UX; avoid its non-recorded replay                              | https://docs.langchain.com/oss/python/langgraph/use-time-travel                                                                                                                                                                                       |
| browser-use                           | `AgentHistoryList` (model output, results, state per step), JSON save with redaction, `rerun_history` re-executes live                                                                                                                                                                                               | Redaction on serialization                                                          | https://deepwiki.com/browser-use/browser-use/2.3-agent-state-and-history-management                                                                                                                                                                   |
| OpenAdapt                             | Recorded ActionEvent / WindowEvent (accessibility info) streams; replay strategies; PII scrubbing providers                                                                                                                                                                                                          | AX-window events, privacy scrubbing                                                 | https://github.com/OpenAdaptAI/OpenAdapt/blob/main/docs/architecture-evolution.md                                                                                                                                                                     |

I didn't find a documented event-sourced or time-travel core in what I checked for Agent S / Simular, Bytebot, Stagehand, Cua or Skyvern; I didn't search X.

If Jared remembers a specific name, the most likely candidates are **OpenHands** (event stream) or **Loom** (deterministic action store plus time-travel inspect).
