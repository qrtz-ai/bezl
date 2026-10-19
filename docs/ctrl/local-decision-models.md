# Ctrl: local "decision" models for computer use (research, 2026-10-17)

Scope: macOS 27 on Apple Silicon, fully offline, with model downloads as the only network use. Numbers below come only from the linked sources. If a cell says "n/f", I didn't find a source for it, so measure it on the Mac.

## 1. What "Jev" means (high confidence)

**Jev is TypeSafe AI's first "System One" model.** It launched on 2026-09-15 and was founded by Diogo Almeida, formerly of OpenAI. The name comes from William Stanley Jevons.

- How it works: unstructured state in, typed calibrated decisions out (Choice / Score / Noul). It doesn't generate text and is trained with RLCD.
- Access: `POST /v1/systemone` only. jev-1.14.0 shipped 2026-10-14 with experimental image input and a `box` bounding-box output type. Context is 64k. Price is $0.042 per M input tokens. Weights are not downloadable.
- Primary sources:
  - https://typesafe.ai/blog/introducing-system-one-models-and-jev
  - https://docs.typesafe.ai/concepts/system-one
  - https://docs.typesafe.ai/models
  - https://typesafe.ai/blog/jev-1-14
  - https://runtimewire.com/article/typesafe-jev-system-one-ai-model-early-access
- **It can't run locally, so it fails the qrtz offline rule.**

Secondary detail:
The most likely match is **TypeSafe Jev**, a "System One" decision model. You give it a state plus typed questions (`choice`, `score`, `noul` = yes/no probability) and it returns typed answers in about 70–500 ms. It's trained with RLCD and is **hosted API only, with no weights released** (as of Sep–Oct 2026).

- https://system-one-models.com/
- https://juliangoldie.com/can-you-run-jev-locally/
- https://shop.zimaspace.com/blogs/tech-ai-hub/what-is-jev-ai-decision-model-agents
- **This breaks the offline rule. Jev itself can't ship in qrtz.**

Local Jev-like options:

- **Laya** (Convai Innovations), Apache-2.0.
  - Built on ModernBERT/mmBERT encoders: 421M English, 322M multilingual, and a 421M typed-decisions checkpoint.
  - One forward pass with no generation, about 33 ms on a T4.
  - Typed-decisions benchmark: 0.766 accuracy vs Jev 1.13.0 at 0.727. That's the publisher's own claim, and Jev leads when there are more than 20 options.
  - Community MLX, GGUF and **Core ML** (FluidInference/laya-coreml) ports exist. FluidInference also publishes the FluidAudio stack we already use.
  - Links: https://huggingface.co/convaiinnovations/laya, https://github.com/NandhaKishorM/laya, https://huggingface.co/convaiinnovations/laya-typed-decisions, https://huggingface.co/models?other=laya
- **OpenJev** (razorback16): reimplements Jev's wire format on DiffusionGemma, with an MLX backend that needs about 16 GB free. Weights are about 18 GB, and it runs 0.2–0.4 s per read on an M3 Ultra. https://jev-ai.pro/model/openjev

The less likely reading is **Meta V-JEPA 2 / V-JEPA 2-AC**. It's an action-conditioned latent world model (a 300M predictor on a frozen video encoder), trained on robot-arm video and used for planning toward goal images. It's a robotics model, not a GUI agent. https://arxiv.org/abs/2506.09985, https://github.com/facebookresearch/vjepa2

**Bottom line:** Jev-style models make _bounded_ decisions (route, classify, confirm or deny, risk score). They don't click buttons. Ctrl needs both kinds of model.

## 1b. Does TypeSafe offer on-prem or local Jev? No.

- The docs serve every Jev version from the hosted endpoint `POST /v1/systemone` only. https://docs.typesafe.ai/models
- The enterprise page lists "Can we run Jev in our own VPC or on-premises?" as an FAQ question but gives no public answer. The enterprise options are zero data retention and SOC 2. https://typesafe.ai/enterprise
- TypeSafe is "working toward" Bedrock, Azure Foundry and Vertex availability, with no date. Those are still cloud. https://help.typesafe.ai/articles/9159694888-do-you-offer-a-dpa-baa-scc-and-specific-data-residency-options
- A third party confirms the terms define the product as the TypeSafe-hosted interface and API. https://tessary.ai/answers/jev/can-i-self-host-jev
- **The only route would be a custom enterprise deal (ask sales@typesafe.ai). Nothing public exists.**

## 1c. Local equivalents: typed, calibrated decisions over a fixed AX-derived action set

Ctrl's decision shape: state = a pruned AX tree plus the goal plus recent history (text). The question is a choice over N enumerated elements and actions, or a yes/no ("is this destructive?", "is the task done?").

| Approach                                                                                                                                                                                                        | Typed?                                 | Gives probabilities?                                                                                          | Latency on Apple Silicon (sourced)                                                                                                                                                                                                                                                                                                                                     | Notes                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         |
| --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------- | ------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **A. Single-pass option scoring with a small LLM in MLX.** Label options A..Z or with numeric ids, run one prefill, read the logits of the label tokens at the answer position, then softmax over the N labels. | yes (argmax over a fixed set)          | **yes, a full distribution**                                                                                  | Cost is about one prefill. MLX prefill on an M3 Max is about 1,160–1,336 tok/s for 680–2,180-token prompts with Qwen3-30B-A3B (llama.cpp: 320–470 tok/s, same machine). So about 1–2 s for a 1.5k-token AX state with that model; small dense 1–4B models are faster but I found no sourced number. Generation: Qwen3-1.7B-4bit at about 290 tok/s (machine unstated). | Closest match to Jev's single-call semantics. mlx-lm exposes `logprobs` (top 1–10) on the server and a `logits_processor` hook; for the full label distribution, call the model directly. A Qwen3 / Gemma / Llama 1–4B class model works. Labels of 255 or fewer, one token each, mirror Jev's cardinality. Sources: https://github.com/ml-explore/mlx-lm/issues/92, https://github.com/ml-explore/mlx-lm/pull/187, https://raw.githubusercontent.com/ml-explore/mlx-lm/main/mlx_lm/SERVER.md |
| **B. Constrained / grammar decoding** (llama.cpp GBNF or JSON schema, an MLX logits_processor mask)                                                                                                             | yes, guaranteed valid                  | only if you read the logprobs of the constrained tokens yourself                                              | as in A, plus a few decode steps                                                                                                                                                                                                                                                                                                                                       | Guarantees a valid action JSON with arguments (e.g. `type(text)`). Use it for actions with free arguments; use A for pure selection. https://github.com/ggml-org/llama.cpp/blob/master/grammars/README.md, https://github.com/ml-explore/mlx-examples/pull/983                                                                                                                                                                                                                                |
| **C. Apple Foundation Models with `@Generable` (enum) plus tools**                                                                                                                                              | yes (constrained decoding, per WWDC25) | **no.** The API does not expose token probabilities; only sampling modes (greedy / random top-k / threshold). | n/f on macOS 27 (nano measurement: 4K context, about 31 s per 1,500-word summary, so generation is slow)                                                                                                                                                                                                                                                               | Zero download and private, but **not calibrated**. Approximating confidence needs k samples and the agreement rate, which multiplies latency. Use it for planning or narration, not gating. https://developer.apple.com/videos/play/wwdc2025/301/, https://developer.apple.com/documentation/foundationmodels/generationoptions/samplingmode, https://antongubarenko.substack.com/p/ios-26-foundation-model-framework-f6d                                                                     |
| **D. Classifier heads on small encoders (ModernBERT / mmBERT): Laya**                                                                                                                                           | yes (choice / score / noul)            | yes, calibrated (RLCD plus temperature scaling; ECE 0.081 claimed vs Jev 0.246)                               | 33 ms per question, 7.2 ms per question batched **on a T4**. **No Apple Silicon number published**, although Core ML, MLX and GGUF ports exist.                                                                                                                                                                                                                        | 512–1,024 token context: too small for a full AX tree, so prune to candidates first. Weak with large option sets (Banking77, 72 labels: 0.425 vs Jev 0.870). Apache-2.0. https://github.com/NandhaKishorM/laya, https://huggingface.co/convaiinnovations/laya, https://huggingface.co/FluidInference/laya-coreml                                                                                                                                                                              |
| **E. OpenJev** (DiffusionGemma, Jev wire format)                                                                                                                                                                | yes                                    | entropy-derived                                                                                               | **0.2–0.4 s per read under MLX on an M3 Ultra**; about 18 GB of weights, 16 GB free RAM                                                                                                                                                                                                                                                                                | Too heavy for an always-on helper. https://jev-ai.pro/model/openjev                                                                                                                                                                                                                                                                                                                                                                                                                           |

**Calibration methods.**

- Temperature scaling: fit one scalar T on a labelled validation set. It's the standard post-hoc fix. https://arxiv.org/abs/1706.04599
- Apply it to the option-softmax from A, measure ECE and Brier on a Ctrl replay set, and set confirm/abstain thresholds from that. Laya's card shows the same practice.
- Verbalized confidence from Apple FM is not a substitute for that calibration.

**Verdict for the "Jev role" in Ctrl:**

1. **Primary: approach A.** Use a Qwen3-class 1.7–4B model in MLX (downloaded with consent), with one prefill per step over the pruned AX list and enumerated action labels. Read the softmax over the labels, then apply temperature scaling. That gives a typed choice with a calibrated probability in one call. If the top-1 probability is below a threshold, ask the user or escalate to the planner.
2. **Fast gates: approach D.** Laya on Core ML for cheap, high-frequency yes/no checks ("destructive?", "done?", "needs confirm?").
3. **Apple FM: approach C** for no-download planning and tool calls. It doesn't gate anything, because it has no probabilities.
4. **Must measure on the Mac:** none of these has a sourced latency on an M-series chip at Ctrl's prompt sizes, except OpenJev and big-model prefill throughput.

## 2. Fallback: GUI action VLMs for apps with poor AX trees (screenshot in, action out)

| Model                                     | Params                   | License                                    | Input                        | OSWorld(-Verified)                | ScreenSpot-Pro                     | Mac notes                                                                                       | Source                                                                                                                                              |
| ----------------------------------------- | ------------------------ | ------------------------------------------ | ---------------------------- | --------------------------------- | ---------------------------------- | ----------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------- |
| UI-TARS-1.5-7B                            | 7B                       | open weights (Apache-2.0 per repo, verify) | screenshot                   | 27.5 (100 steps)                  | 49.6 (repo) / 61.6 (leaderboard)   | Qwen2.5-VL base, MLX/GGUF ports exist                                                           | https://github.com/bytedance/UI-TARS/                                                                                                               |
| UI-TARS-72B-DPO                           | 72B                      | open                                       | screenshot                   | 24.6                              | 38.1                               | too big for most Macs                                                                           | same                                                                                                                                                |
| OpenCUA-7B / 32B / 72B                    | 7/32/72B                 | open (license n/f)                         | screenshot                   | 26.6 / 34.8 / 45.0                | 72B: 60.8                          | 7B is feasible                                                                                  | https://arxiv.org/html/2601.15876 (table)                                                                                                           |
| **EvoCUA-8B / 32B**                       | 8/32B                    | open weights                               | screenshot                   | **46.1 / 56.7** (50 steps)        | n/f                                | 8B is the best single small native model in the table                                           | https://arxiv.org/html/2601.15876                                                                                                                   |
| Qwen3-VL-8B-Thinking / 32B-Thinking       | 8/32B                    | Apache-2.0                                 | screenshot                   | 30.6 / 41.0                       | 8B: 54.6, 4B: 59.5                 | Qwen3-VL-8B 4-bit on **M4 Max: 45.2 tok/s avg, 14.1 tok/s at a 1080p image**; M1 Max 21.1 tok/s | https://arxiv.org/html/2601.15876, https://vllm-mlx.is-a.dev/benchmarks/image/, https://www.sota2.com/research/sota/gui-grounding-on-screenspot-pro |
| GUI-Owl-1.5-8B                            | 8B                       | open                                       | screenshot                   | (7B-Desktop-RL: 34.9)             | 68.88                              |                                                                                                 | sota2 leaderboard above                                                                                                                             |
| UI-Venus-1.5-8B / MAI-UI-8B / Step-GUI-4B | 8/8/4B                   | open weights                               | screenshot                   | n/f                               | 68.4 / 65.8 / 60.0                 | strong grounders                                                                                | sota2 leaderboard                                                                                                                                   |
| Holo2-8B / 30B-A3B                        | 8B / 30B MoE (3B active) | open weights                               | screenshot                   | OSWorld-G 76.1 (30B)              | 58.9 / 66.1                        | MoE with 3B active is Mac-friendly                                                              | https://hcompany.ai/holo2                                                                                                                           |
| GTA1-7B / 32B                             | 7/32B                    | open                                       | screenshot (grounder)        | used as the grounder in LocalLSTC | 50.1 / 63.6                        |                                                                                                 | sota2 leaderboard                                                                                                                                   |
| GUI-Actor-7B                              | 7B                       | MIT                                        | screenshot (grounder)        | —                                 | 40.7 (Qwen2-VL), 44.6 (Qwen2.5-VL) |                                                                                                 | https://huggingface.co/microsoft/GUI-Actor-7B-Qwen2-VL                                                                                              |
| ShowUI-2B                                 | 2B                       | MIT                                        | screenshot                   | —                                 | 7.7                                | outdated                                                                                        | https://huggingface.co/showlab/ShowUI-2B                                                                                                            |
| Fara-7B → **Fara1.5-4B/9B/27B**           | 4/9/27B                  | open (MIT per repo, verify)                | screenshot, **browser only** | not evaluated on desktop          | —                                  | Online-Mind2Web 57.3 / 63.4 / 72.3                                                              | https://github.com/microsoft/fara                                                                                                                   |
| Apple Ferret-UI Lite                      | 3B                       | weights not confirmed public               | screenshot                   | —                                 | 53.3                               | Apple's on-device design reference                                                              | https://arxiv.org/html/2509.26539                                                                                                                   |

Agent harnesses that matter:

- **LocalLSTC** (Aug 2026, training-free): a Qwen3.5-9B planner plus a GTA1-7B grounder. A Qwen3.8-27B configuration reached 73.7% on OSWorld. https://arxiv.org/html/2608.25777
- **Local CUA inference-time scaling study:** Qwen3-VL-8B/30B-A3B and UI-TARS-1.5-7B as local planners and judges. https://arxiv.org/html/2607.28573v1
- **MLX vs GGUF on an M5 Max:** MLX is faster and more accurate for VLM grounding, and 4-bit keeps nearly all the quality. https://github.com/ivanfioravanti/vlm-bakeoff

## 3. Apple Foundation Models (macOS 27)

- The on-device `SystemLanguageModel` supports **tool calling** with `Tool` structs and `GenerationOptions` tool modes (allowed / disallowed / required).
- It also does guided generation (`@Generable`), and WWDC26 material mentions image input.
- It's zero-download and private, but it's a small 4K-context model (our measurement from nano). Treat it as a planner over the AX tree, not a pixel grounder.
- Sources: https://developer.apple.com/foundation-models-framework/, https://developer.apple.com/videos/play/wwdc2026/241/, https://developer.apple.com/videos/play/wwdc2026/242/
- I haven't independently verified the image-input claim. Confirm it in the docs before relying on it.

## 4. AX tree vs screenshot

- The macOS Accessibility API gives exact roles, labels and frames for free. Text over the tree is cheap: a small LLM or Apple FM with tools can plan over it with no vision cost.
- Screenshots are needed for apps with poor AX support (Electron, canvas, games). At a 1080p screenshot, decoding speed drops to about 14 tok/s on an M4 Max.
- Hybrid designs (planner plus grounder) lead the open results: LocalLSTC, and the EvoCUA table vs single models.

## 5. Recommendation for Ctrl v1 (aligned with decision-layer.md)

1. **Hierarchical small decisions, AX-first:** choose_app → choose_move (shortcut or AX branch) → choose_element_action / choose_text → verify. Keep options per node at 20 or fewer.
2. **Default node scorer:** a small Qwen3 (1.7–4B, 4-bit) in MLX, using option scoring (softmax over label logits), with temperature scaling and act / backtrack / ask thresholds.
3. **Fast yes/no gates** (verify, confirm-gate, simple choices): Laya on Core ML.
4. **Apple Foundation Models only as the multi-step planner.** It exposes no probabilities, so it never gates.
5. **Fallback for poor or flat trees:** group and summarize, then OCR labels, then the screenshot locator (Holo2-30B-A3B, or Step-GUI-4B / Qwen3-VL-4B), downloaded with consent.
6. **A confirm gate before irreversible actions:** a rules table OR Laya ≥ 0.3.
7. **Fully offline;** model downloads only from settings (ADR 0163).
8. **Benchmark** the hierarchical chain against flat scoring on 20 tasks on the Mac before locking this in (decision-layer.md §12).
