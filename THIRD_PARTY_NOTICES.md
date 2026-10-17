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
| LocalVQE v1.3 (through FluidAudio)                                                                                                                     | Mic echo cancellation                             | See the model card        | Check the license on the FluidAudio model card before release (listed in the backlog).                                                                                                                                         |

## Native and runtime libraries

| Component                                                              | Used for                                                                                                           | License                                                                                |
| ---------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------ | -------------------------------------------------------------------------------------- |
| FluidAudio (github.com/FluidInference/FluidAudio, 0.17.x)              | Core ML ASR, diarization and VQE in the Swift helper                                                               | Apache-2.0                                                                             |
| Electron (includes Chromium and Node.js)                               | App runtime                                                                                                        | MIT (Chromium and Node notices ship in `LICENSES.chromium.html` inside the app bundle) |
| onnxruntime-node / onnxruntime-web / onnxruntime-common                | Embedding inference (native, with a WASM fallback)                                                                 | MIT, © Microsoft                                                                       |
| @huggingface/transformers, @huggingface/tokenizers                     | Model loading and tokenization                                                                                     | Apache-2.0                                                                             |
| sharp                                                                  | Bundled only as a JS stub that transformers.js imports; the native libvips binaries (LGPL-3.0) are **not** shipped | Apache-2.0                                                                             |
| Apple frameworks (ScreenCaptureKit, Core Audio, Speech, Accessibility) | Capture and the AX speaker signal                                                                                  | Apple SDK terms (system frameworks, not redistributed)                                 |

## npm production dependencies

Generated with `node scripts/third-party.mjs` (the dependency tree of `apps/desktop` without dev dependencies). Most of it is bundled into the app by Vite.

| Package                          | Version                       | License           |
| -------------------------------- | ----------------------------- | ----------------- |
| @emnapi/runtime                  | 1.11.3                        | MIT               |
| @huggingface/jinja               | 0.5.10                        | MIT               |
| @huggingface/tokenizers          | 0.2.0                         | Apache-2.0        |
| @huggingface/transformers        | 4.3.1                         | Apache-2.0        |
| @img/colour                      | 1.1.0                         | MIT               |
| @img/sharp-libvips-linux-x64     | 1.3.4                         | LGPL-3.0-or-later |
| @img/sharp-libvips-linuxmusl-x64 | 1.3.4                         | LGPL-3.0-or-later |
| @img/sharp-linux-x64             | 0.35.5                        | Apache-2.0        |
| @img/sharp-linuxmusl-x64         | 0.35.5                        | Apache-2.0        |
| @protobufjs/aspromise            | 1.1.2                         | BSD-3-Clause      |
| @protobufjs/base64               | 1.1.2                         | BSD-3-Clause      |
| @protobufjs/codegen              | 2.0.5                         | BSD-3-Clause      |
| @protobufjs/eventemitter         | 1.1.1                         | BSD-3-Clause      |
| @protobufjs/fetch                | 1.1.1                         | BSD-3-Clause      |
| @protobufjs/float                | 1.0.2                         | BSD-3-Clause      |
| @protobufjs/path                 | 1.1.2                         | BSD-3-Clause      |
| @protobufjs/pool                 | 1.1.0                         | BSD-3-Clause      |
| @protobufjs/utf8                 | 1.1.2                         | BSD-3-Clause      |
| @types/node                      | 22.20.5                       | MIT               |
| adm-zip                          | 0.6.1                         | MIT               |
| buffer-crc32                     | 1.0.0                         | MIT               |
| debug                            | 2.6.9                         | MIT               |
| define-data-property             | 1.1.4                         | MIT               |
| define-properties                | 1.2.1                         | MIT               |
| detect-libc                      | 2.1.2                         | Apache-2.0        |
| electron-squirrel-startup        | 1.0.1                         | Apache-2.0        |
| es-define-property               | 1.0.1                         | MIT               |
| es-errors                        | 1.3.0                         | MIT               |
| escape-string-regexp             | 4.0.0                         | MIT               |
| flatbuffers                      | 25.9.23                       | Apache-2.0        |
| github-url-to-object             | 4.0.6                         | MIT               |
| global-agent                     | 4.1.3                         | BSD-3-Clause      |
| globalthis                       | 1.0.4                         | MIT               |
| gopd                             | 1.2.0                         | MIT               |
| guid-typescript                  | 1.0.9                         | ISC               |
| has-property-descriptors         | 1.0.2                         | MIT               |
| is-url                           | 1.2.4                         | MIT               |
| long                             | 5.3.2                         | Apache-2.0        |
| matcher                          | 4.0.0                         | MIT               |
| ms                               | 2.0.0                         | MIT               |
| ms                               | 2.1.3                         | MIT               |
| object-keys                      | 1.1.1                         | MIT               |
| onnxruntime-common               | 1.30.0                        | MIT               |
| onnxruntime-common               | 1.31.0-dev.20260911-2a43ec07e | MIT               |
| onnxruntime-node                 | 1.30.0                        | MIT               |
| onnxruntime-web                  | 1.31.0-dev.20260914-8d85527a0 | MIT               |
| pend                             | 1.2.0                         | MIT               |
| platform                         | 1.3.6                         | MIT               |
| protobufjs                       | 7.6.6                         | BSD-3-Clause      |
| react                            | 19.3.1                        | MIT               |
| react-dom                        | 19.3.1                        | MIT               |
| scheduler                        | 0.28.0                        | MIT               |
| semver                           | 7.8.5                         | ISC               |
| serialize-error                  | 8.1.0                         | MIT               |
| sharp                            | 0.35.5                        | Apache-2.0        |
| tslib                            | 2.8.1                         | 0BSD              |
| type-fest                        | 0.20.2                        | (MIT OR CC0-1.0)  |
| undici-types                     | 6.21.0                        | MIT               |
| update-electron-app              | 3.3.0                         | MIT               |
| yauzl                            | 3.4.0                         | MIT               |
| yazl                             | 3.3.1                         | MIT               |
| zod                              | 4.6.7                         | MIT               |
