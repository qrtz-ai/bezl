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
