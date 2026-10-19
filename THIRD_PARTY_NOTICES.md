# Third-party notices

qrtz ships or downloads the components below. Each one keeps its own license. Models are **not** in the
app bundle. They download on first use (from Hugging Face, through transformers.js or FluidAudio) and are
cached in the user's Application Support folder.

## Models (downloaded at runtime)

| Model                                                                                                                                                  | Used for                                          | License                   | Attribution / notes                                                                                                                                                                                                            |
| ------------------------------------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------- | ------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| NVIDIA Parakeet Realtime EOU 120M (`nvidia/parakeet_realtime_eou_120m-v1`; Core ML conversion `FluidInference/parakeet-realtime-eou-120m-coreml`)      | Default live streaming ASR (`parakeet-eou-160ms`) | NVIDIA Open Model License | Commercial use is allowed. Keep the license notice and attribute: "Licensed by NVIDIA Corporation under the NVIDIA Open Model License". https://www.nvidia.com/en-us/agreements/enterprise-software/nvidia-open-model-license/ |
| NVIDIA Parakeet 0.6B unified (`FluidInference/parakeet-unified-en-0.6b-coreml`)                                                                        | Higher-accuracy ASR option                        | CC-BY-4.0                 | Attribution: NVIDIA (Parakeet), Core ML conversion by FluidInference. https://creativecommons.org/licenses/by/4.0/                                                                                                             |
| NVIDIA Streaming Sortformer 4spk v2 (`nvidia/diar_streaming_sortformer_4spk-v2`; Core ML conversion `FluidInference/diar-streaming-sortformer-coreml`) | Speaker diarization                               | CC-BY-4.0                 | Attribution: NVIDIA (Sortformer), Core ML conversion by FluidInference. Do not use v1 checkpoints, which were CC-BY-NC.                                                                                                        |
| all-MiniLM-L6-v2 (`sentence-transformers/all-MiniLM-L6-v2`, ONNX build `Xenova/all-MiniLM-L6-v2`)                                                      | On-device semantic search embeddings              | Apache-2.0                | © sentence-transformers authors.                                                                                                                                                                                               |
| LocalVQE v1.3 (`LocalAI-io/LocalVQE`; Core ML conversion `FluidInference/localvqe-coreml`)                                                             | Mic echo cancellation                             | Apache-2.0                | © LocalVQE contributors; Core ML conversion by FluidInference. https://huggingface.co/FluidInference/localvqe-coreml                                                                                                           |

## Models published as qrtz packages (ADR 0157 §6)

The package manager installs these pinned model sets (`crates/qrtz-app/src/local_models.json`,
`crates/qrtz-app/src/ctrl_laya_model.json`) from `modules.qrtz.ai` and its B2 mirror, with the pinned Hugging Face
URL as a second source. Hosting them is redistribution, so each license's notice terms apply to the hosted copies;
this file (shipped with the app) carries them. Checked 2026-10-18 against the model cards and the Hugging Face API.

| Package                    | Upstream (pinned revision)                                                                                              | License                                                                                                            | Notice / attribution                                                                                                                                                                                                                                 |
| -------------------------- | ----------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `model.parakeet-tdt-v2-en` | `FluidInference/parakeet-tdt-0.6b-v2-coreml` @ `ee09c569`, from `nvidia/parakeet-tdt-0.6b-v2`                           | CC-BY-4.0 (https://creativecommons.org/licenses/by/4.0/)                                                           | "Parakeet TDT 0.6B V2" by NVIDIA, Core ML conversion by FluidInference, used under CC-BY-4.0; converted to Core ML (not otherwise modified).                                                                                                         |
| `model.nemotron`           | `FluidInference/nemotron-speech-streaming-en-0.6b-coreml` @ `e673531c`, from `nvidia/nemotron-speech-streaming-en-0.6b` | NVIDIA Open Model License (https://www.nvidia.com/en-us/agreements/enterprise-software/nvidia-open-model-license/) | Licensed by NVIDIA Corporation under the NVIDIA Open Model License. A copy of the agreement must accompany the hosted copies (§3.1). (The conversion's README says Apache 2.0; its license metadata and the upstream say NVIDIA OML, which governs.) |
| `model.nemotron-diarizer`  | `FluidInference/nemotron-3-diarization-coreml` @ `25a90f97`, from `nvidia/Nemotron-3-Diarization`                       | OpenMDW-1.1 (https://openmdw.ai/license/)                                                                          | Nemotron 3 Diarization © NVIDIA; Core ML conversion by Fluid Inference. A copy of OpenMDW-1.1 must accompany the hosted copies.                                                                                                                      |
| `model.localvqe`           | `FluidInference/localvqe-coreml` @ `42054307`, from `LocalAI-io/LocalVQE`                                               | Apache-2.0                                                                                                         | LocalVQE (localai-org/LocalVQE), Core ML conversion by FluidInference.                                                                                                                                                                               |
| `model.laya`               | `FluidInference/laya-coreml` @ `7b8d7a2b`, from `convaiinnovations/laya` (multilingual @ `1c5edc17`)                    | Apache-2.0 (tokenizer from `jhu-clsp/mmBERT-base`, MIT)                                                            | Weights by Convai Innovations and contributors; Core ML conversion by Fluid Inference, not an official Convai Innovations release. Upstream NOTICE: https://huggingface.co/FluidInference/laya-coreml/blob/main/NOTICE.md                            |

## Native and runtime libraries

| Component                                                              | Used for                                                                                                           | License                                                                                         |
| ---------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------ | ----------------------------------------------------------------------------------------------- |
| FluidAudio (github.com/FluidInference/FluidAudio, 0.17.x)              | Core ML ASR, diarization and VQE in the Swift helper                                                               | Apache-2.0                                                                                      |
| Electron (includes Chromium and Node.js)                               | App runtime                                                                                                        | MIT (Chromium and Node notices ship in `LICENSES.chromium.html` inside the app bundle)          |
| onnxruntime-node / onnxruntime-web / onnxruntime-common                | Embedding inference (native, with a WASM fallback)                                                                 | MIT, © Microsoft                                                                                |
| @huggingface/transformers, @huggingface/tokenizers                     | Model loading and tokenization                                                                                     | Apache-2.0                                                                                      |
| sharp                                                                  | Bundled only as a JS stub that transformers.js imports; the native libvips binaries (LGPL-3.0) are **not** shipped | Apache-2.0                                                                                      |
| Geist Pixel (Circle), from the `geist` npm package (vercel/geist-font) | The app's single UI font, bundled as `packages/ui/src/fonts/GeistPixel-Circle.woff2`                               | SIL OFL-1.1, © 2023 Vercel with basement.studio (`packages/ui/src/fonts/OFL-GeistPixel.txt`)    |
| Feather icons (`react-feather` 2.0.10, feathericons.com)               | Every icon in the UI, through the `Icon` component in `@qrtz/ui`                                                   | MIT, © 2017 Carmelo Pullara (react-feather); icon artwork MIT, © 2013-2017 Cole Bemis (feather) |
| Apple frameworks (ScreenCaptureKit, Core Audio, Speech, Accessibility) | Capture and the AX speaker signal                                                                                  | Apple SDK terms (system frameworks, not redistributed)                                          |

## JavaScript production dependencies

Generated with `bun scripts/third-party.mjs` (the `dependencies` tree of `apps/desktop`, without dev dependencies). Vite bundles it into the app's renderer.

| Package         | Version | License           |
| --------------- | ------- | ----------------- |
| @tauri-apps/api | 2.12.4  | Apache-2.0 OR MIT |
| js-tokens       | 4.0.0   | MIT               |
| loose-envify    | 1.4.0   | MIT               |
| object-assign   | 4.1.1   | MIT               |
| prop-types      | 15.8.1  | MIT               |
| react           | 19.3.1  | MIT               |
| react-dom       | 19.3.1  | MIT               |
| react-feather   | 2.0.10  | MIT               |
| react-is        | 16.13.1 | MIT               |
| scheduler       | 0.28.0  | MIT               |
| zod             | 4.6.7   | MIT               |

## Ported source: Spectacle (qrtz snap, ADR 0159)

`native/qrtz-helper/Sources/QrtzSnap` ports Spectacle's window position calculations, screen detection, history,
movers and default shortcuts (https://github.com/eczarny/spectacle). The notice is also kept in
`SnapGeometry.swift`.

> Copyright (c) 2017 Eric Czarny <eczarny@gmail.com>
>
> Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated
> documentation files (the "Software"), to deal in the Software without restriction, including without limitation
> the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to
> permit persons to whom the Software is furnished to do so, subject to the following conditions:
>
> The above copyright notice and this permission notice shall be included in all copies or substantial portions of
> the Software.
>
> THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO
> THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
> AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT,
> TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
> SOFTWARE.
