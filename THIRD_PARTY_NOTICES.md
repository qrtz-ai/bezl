# Third-party notices

> **Draft for legal review by Jared before release.** Generated from the code on 2026-10-18: the pinned model
> manifests (`crates/qrtz-app/src/local_models.json`, `crates/qrtz-app/src/ctrl_laya_model.json`, the module
> manifests in `modules/*/module.json`), `native/qrtz-helper/Package.swift`, `Cargo.lock` and `bun.lock`. Each
> model's license was read from its Hugging Face API entry (`cardData.license`) and model card **at the pinned
> revision**, and from the LICENSE / NOTICE files in that revision where the repository has them. It is not legal
> advice. Items marked **Review** need a decision before release.

qrtz ships the code below inside `qrtz.app`, and downloads the models below only after the user says yes (settings →
speech, switching echo or nano on in the module manager, ctrl's own model setting, or a confirmed install; ADR 0163).
Every component keeps its own license. This file ships in the app (`Contents/Resources/THIRD_PARTY_NOTICES.md`) and is
shown in settings → system → "open-source licenses".

## Source code

qrtz is AGPL-3.0-only (`Contents/Resources/licenses/AGPL-3.0.txt`, with the additional permission in
`licenses/LICENSE-EXCEPTION.md`). Every release attaches its complete source next to the download, at
`https://modules.qrtz.ai/v1/app/<version>/`:

- `qrtz_<version>_source.tar.gz`: qrtz at the release commit (tag `app-v<version>` on
  https://github.com/qrtz-ai/qrtz);
- `qrtz_<version>_fluidaudio_source.tar.gz`: FluidAudio at exactly the pinned version and revision (0.17.7,
  `503b4bd1`), including its possibly espeak-derived GPL-3.0-or-later parts;
- `qrtz_<version>_build-scripts.tar.gz`: the build and release scripts and the manifests and lockfiles they read;
- `qrtz_<version>_source.sha256`: their checksums.

The same source is on GitHub at the `app-v<version>` tag and at
https://github.com/FluidInference/FluidAudio/tree/v0.17.7.

## Models (downloaded after the user's yes, not in the app bundle)

Today each file is fetched from Hugging Face at the pinned commit below, checked against its pinned sha256, and
installed read-only. The files are used as published, with no further change by qrtz. Once the package hosts
(`modules.qrtz.ai` and its mirror) serve these models, qrtz **redistributes** them. Each package must then carry the
model's license text and the notices below (NVIDIA Open Model License §Redistribution, OpenMDW-1.1 §1, Apache-2.0 §4,
CC-BY-4.0 §3).

| Model (qrtz package)                                                 | Used for                                 | License                                                                                                                        | Source, pinned revision (verified)                                                                                                                                                                   | Upstream (license there)                                                                                                                                                                                                           |
| -------------------------------------------------------------------- | ---------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Parakeet TDT 0.6B v2, Core ML (`model.parakeet-tdt-v2-en`)           | echo's dictation (speech to text)        | [CC-BY-4.0](https://creativecommons.org/licenses/by/4.0/legalcode)                                                             | [FluidInference/parakeet-tdt-0.6b-v2-coreml @ `ee09c569`](https://huggingface.co/FluidInference/parakeet-tdt-0.6b-v2-coreml/tree/ee09c569f73759e6d44c9bd16766f477b2b36d39)                           | [nvidia/parakeet-tdt-0.6b-v2](https://huggingface.co/nvidia/parakeet-tdt-0.6b-v2) (CC-BY-4.0)                                                                                                                                      |
| Nemotron Speech Streaming EN 0.6B, 560 ms Core ML (`model.nemotron`) | nano's meeting transcription             | [NVIDIA Open Model License](https://www.nvidia.com/en-us/agreements/enterprise-software/nvidia-open-model-license/)            | [FluidInference/nemotron-speech-streaming-en-0.6b-coreml @ `e673531c`](https://huggingface.co/FluidInference/nemotron-speech-streaming-en-0.6b-coreml/tree/e673531caa6d25ab7baf5a8c14c9b99ba1551838) | [nvidia/nemotron-speech-streaming-en-0.6b](https://huggingface.co/nvidia/nemotron-speech-streaming-en-0.6b) (NVIDIA Open Model License)                                                                                            |
| Nemotron 3 Diarization, fast32 Core ML (`model.nemotron-diarizer`)   | nano's speaker labels                    | [OpenMDW-1.1](https://openmdw.ai/license/1-1/)                                                                                 | [FluidInference/nemotron-3-diarization-coreml @ `25a90f97`](https://huggingface.co/FluidInference/nemotron-3-diarization-coreml/tree/25a90f97f254428d4b30374b76af9c74fdee8327)                       | [nvidia/Nemotron-3-Diarization](https://huggingface.co/nvidia/Nemotron-3-Diarization) (OpenMDW-1.1)                                                                                                                                |
| LocalVQE v1.3 4.8M 16 ms, Core ML (`model.localvqe`)                 | nano's mic echo cancellation             | [Apache-2.0](https://www.apache.org/licenses/LICENSE-2.0)                                                                      | [FluidInference/localvqe-coreml @ `42054307`](https://huggingface.co/FluidInference/localvqe-coreml/tree/4205430781c240397073d6f7bf676fb4c732fc12) (LICENSE: Apache-2.0)                             | [LocalAI-io/LocalVQE](https://huggingface.co/LocalAI-io/LocalVQE) (Apache-2.0)                                                                                                                                                     |
| Laya multilingual e8 L512, Core ML (`model.laya`)                    | ctrl's decision model (opt-in, ADR 0116) | [Apache-2.0](https://www.apache.org/licenses/LICENSE-2.0); `tokenizer.json` [MIT](https://huggingface.co/jhu-clsp/mmBERT-base) | [FluidInference/laya-coreml @ `7b8d7a2b`](https://huggingface.co/FluidInference/laya-coreml/tree/7b8d7a2b7e28e746c6ecaad44bbcd5cf251a4fcc) (LICENSE: Apache-2.0, NOTICE.md)                          | [convaiinnovations/laya @ `1c5edc17`](https://huggingface.co/convaiinnovations/laya/tree/1c5edc17a7acd8701df6fc341c0d179f1c62c982) (Apache-2.0); encoder [jhu-clsp/mmBERT-base](https://huggingface.co/jhu-clsp/mmBERT-base) (MIT) |

Modification: every model above is the upstream checkpoint **converted to Core ML by FluidInference** (Fluid
Inference); the Laya conversion also changes tensor precision and fixes input shapes (its NOTICE.md). qrtz makes no
further change.

Not covered by this table: Apple's on-device speech (SpeechAnalyzer) and Apple Intelligence summaries
(FoundationModels) are macOS system frameworks and models; qrtz neither ships nor downloads them. The developer-only
`echo-bench` script (`native/qrtz-helper/scripts/echo-bench-models.json`) downloads extra benchmark models on a
developer's Mac; it is not reachable from the app's UI and those models are not listed here.

### Required attribution text

**Parakeet TDT 0.6B v2** (CC-BY-4.0, §3(a)):

> "parakeet-tdt-0.6b-v2" by NVIDIA Corporation, https://huggingface.co/nvidia/parakeet-tdt-0.6b-v2, licensed under
> CC BY 4.0 (https://creativecommons.org/licenses/by/4.0/). Converted to Core ML by FluidInference
> (https://huggingface.co/FluidInference/parakeet-tdt-0.6b-v2-coreml). Provided as is, without warranties.

**Nemotron Speech Streaming EN 0.6B** (NVIDIA Open Model License, Redistribution (a)):

> Licensed by NVIDIA Corporation under the NVIDIA Open Model License.

Core ML conversion by FluidInference. When qrtz redistributes the model, give recipients a copy of the agreement
(https://www.nvidia.com/en-us/agreements/enterprise-software/nvidia-open-model-license/) and include the sentence
above in a "Notice" text file with the copies. **Review:** at `e673531c` the FluidInference model card's metadata
says `nvidia-open-model-license`, but its "License" section says "Apache 2.0 (following NVIDIA's original
license)". The upstream NVIDIA card is the NVIDIA Open Model License, so this file follows it.

**Nemotron 3 Diarization** (OpenMDW-1.1, §1):

> Nemotron-3-Diarization by NVIDIA Corporation (https://huggingface.co/nvidia/Nemotron-3-Diarization), licensed under
> the OpenMDW License Agreement, version 1.1 (https://openmdw.ai/license/1-1/). Core ML conversion by Fluid Inference
> (https://huggingface.co/FluidInference/nemotron-3-diarization-coreml), under the same license.

When redistributing, keep a copy of the OpenMDW agreement and every copyright and origin notice in the model files.

**LocalVQE v1.3** (Apache-2.0, §4):

> LocalVQE by LocalAI-io (https://huggingface.co/LocalAI-io/LocalVQE), licensed under the Apache License, Version 2.0.
> Core ML conversion by FluidInference (https://huggingface.co/FluidInference/localvqe-coreml), Apache-2.0.

The upstream card notes that the weights were trained on the ICASSP 2023 DNS Challenge data (Microsoft, CC BY 4.0)
and fine-tuned on the ICASSP 2022/2023 AEC Challenge. **Review:** neither repository names a copyright holder
beyond "LocalAI-io"; the attribution above uses the repository owner.

**Laya multilingual** (Apache-2.0, §4(d): the NOTICE is reproduced as published):

> **NOTICE.md (FluidInference/laya-coreml)**
>
> ## This repository
>
> Core ML conversion of the laya-multilingual decision model, by Fluid Inference.
> Licensed under the Apache License, Version 2.0; see `LICENSE`.
>
> ## Upstream model and code
>
> The converted weights and the reference implementation are the work of
> Convai Innovations and contributors, used under the Apache License, Version 2.0.
>
> - Weights: [`convaiinnovations/laya`](https://huggingface.co/convaiinnovations/laya),
>   subfolder `multilingual`, revision `1c5edc17a7acd8701df6fc341c0d179f1c62c982`.
> - Reference implementation: [`NandhaKishorM/laya`](https://github.com/NandhaKishorM/laya).
>
> The encoder is [`jhu-clsp/mmBERT-base`](https://huggingface.co/jhu-clsp/mmBERT-base),
> and `tokenizer.json` carries its vocabulary, redistributed under that model's terms.
>
> No weights were retrained. The conversion changes tensor precision and fixes input
> shapes; the parameters are the upstream ones. This is an independent conversion and
> is not an official Convai Innovations release.

**Review:** `tokenizer.json` carries the vocabulary of jhu-clsp/mmBERT-base (MIT per its card). That repository has
no LICENSE file or copyright line, so the MIT copyright notice to reproduce is unknown; ask the authors or treat the
card as the notice.

## Code inside qrtz.app

| Component                                                                                                                         | Where it runs                                                       | License                                                                                                     | Notes                                                                                                                                                                             |
| --------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| [FluidAudio](https://github.com/FluidInference/FluidAudio) 0.17.7 (`Package.swift`: `exact: "0.17.7"`)                            | The Swift helper (statically linked): ASR, diarization, VQE runtime | [Apache-2.0](https://github.com/FluidInference/FluidAudio/blob/v0.17.7/LICENSE) (verified at tag `v0.17.7`) | Its LICENSE carries no copyright line (the appendix template) and the tag has no NOTICE. Copyright FluidInference.                                                                |
| fastcluster (inside FluidAudio, `Sources/FastClusterWrapper`)                                                                     | The Swift helper                                                    | BSD-2-Clause                                                                                                | © 2011 Daniel Müllner; changes from 1.1.24 on © Google Inc. Notice below.                                                                                                         |
| [text-processing-rs](https://github.com/FluidInference/text-processing-rs) v0.3.1 (FluidAudio's `NemoTextProcessing.xcframework`) | The Swift helper                                                    | Apache-2.0 (LICENSE, NOTICE and THIRD-PARTY-LICENSES.md at `v0.3.1`)                                        | Copyright 2026 FluidInference; a port of NVIDIA NeMo Text Processing (Apache-2.0, © NVIDIA CORPORATION & AFFILIATES); links rustfst and flate2 (MIT OR Apache-2.0). NOTICE below. |
| Tauri 2, wry, tao and the Rust crates listed below                                                                                | The app, the `qrtz` command, the helpers' hosts                     | See the generated list                                                                                      | 377 crates, all permissive or weak-copyleft (MPL-2.0); none is GPL/LGPL.                                                                                                          |
| The JavaScript packages listed below                                                                                              | The renderer (bundled by Vite)                                      | MIT / Apache-2.0                                                                                            |                                                                                                                                                                                   |
| Geist Pixel (Square), from the `geist` npm package (vercel/geist-font)                                                            | The UI font, `packages/ui/src/fonts/GeistPixel-Square.woff2`        | SIL OFL-1.1, © 2023 Vercel with basement.studio                                                             | The license ships next to it (`packages/ui/src/fonts/OFL-GeistPixel.txt`).                                                                                                        |
| Feather icons (`react-feather` 2.0.10, feathericons.com)                                                                          | Every icon, through `@qrtz/ui` `Icon`                               | MIT, © 2017 Carmelo Pullara (react-feather); artwork MIT, © 2013-2017 Cole Bemis (feather)                  |                                                                                                                                                                                   |
| Spectacle (ported, qrtz snap)                                                                                                     | `native/qrtz-helper/Sources/QrtzSnap`                               | MIT, © 2017 Eric Czarny                                                                                     | Notice below.                                                                                                                                                                     |
| Apple frameworks (ScreenCaptureKit, Core Audio, Speech, Accessibility, Core ML, FoundationModels)                                 | The Swift helper                                                    | Apple SDK terms                                                                                             | System frameworks, not redistributed.                                                                                                                                             |

MPL-2.0 crates (`cssparser`, `cssparser-macros`, `dtoa-short`, `selectors`, `option-ext`) are used unmodified from
crates.io; their source is available there, which satisfies MPL-2.0 §3.2 for the executable. `webpki-roots` is
CDLA-Permissive-2.0 (Mozilla's root list), and the Unicode-3.0 crates (ICU4X data) require their notice, which the
list below carries by name. **Review:** a full-text bundle of every license (e.g. `cargo about generate`) is not
generated yet. The table names each license; the full texts are at https://spdx.org/licenses/.

### espeak-ng (GPL-3.0-or-later): possibly espeak-derived parts of FluidAudio's LuxTTS

[eSpeak NG](https://github.com/espeak-ng/espeak-ng) is GPL-3.0-or-later, Copyright (C) 2005 to 2014 Jonathan
Duddington, Copyright (C) 2015-2017 Reece H. Dunn, and contributors. qrtz does not ship, link or call espeak-ng
itself. FluidAudio 0.17.7's LuxTTS English G2P (`Sources/FluidAudio/TTS/LuxTts/G2p`) is possibly espeak-derived:
FluidAudio says it reproduces espeak-ng's output, with clause rules modelled on espeak's `translate.c` /
`dictionary.c` and a lexicon generated offline by running espeak-ng (`luxtts_en_us_lexicon.tsv.zz`,
`luxtts_en_us_g2p_aux.json`). qrtz treats those rules and any data generated from espeak-ng as GPL-3.0-or-later.

What `qrtz.app` contains: `qrtz-echo` links FluidAudio statically, so the G2P code (its rules and a few phoneme
constants) is compiled in. The generated lexicon and aux tables are not: they are only in FluidAudio's SwiftPM
resource bundle, which the app doesn't ship (`scripts/release/verify-bundle.sh` refuses any nested `.bundle`), and no
table from them is compiled into `qrtz-echo`. qrtz never uses text-to-speech.

The GPL-3.0 text ships in the app (`Contents/Resources/licenses/GPL-3.0.txt`; repo `licenses/GPL-3.0.txt`). The
source of these parts is FluidAudio at tag `v0.17.7`, which every qrtz release attaches (see "Source code" below), and
espeak-ng itself. GPL-3.0 and qrtz's AGPL-3.0-only may be combined (section 13 of each license); each part keeps its
own license, and [LICENSE-EXCEPTION.md](LICENSE-EXCEPTION.md) does not apply to them.

### FluidAudio's own third-party code and data

FluidAudio 0.17.7 carries these in its `ThirdPartyLicenses/` (each license below was taken from the upstream project,
not from FluidAudio's copy). They are compiled into `qrtz-echo` with FluidAudio (strings from all of them except
num2kana and the unidic-lite dictionary appear in the CI build of `qrtz-echo`); the dictionaries and word lists are in
FluidAudio's resource bundle or model downloads, which `qrtz.app` does not ship. qrtz doesn't use FluidAudio's
text-to-speech. The Apache-2.0 text is in `licenses/Apache-2.0.txt` (in the app: `Contents/Resources/licenses/`).

- **VBx** (speaker-diarization clustering), [BUTSpeechFIT/VBx](https://github.com/BUTSpeechFIT/VBx): Apache-2.0
  (its README and file headers, e.g. `VBx/VBx.py`: "Copyright 2021 Lukas Burget, Mireia Diez"). FluidAudio: "Copyright
  2021-2024 BUT Speech@FIT (original VBx project)".
- **Misaki** (Japanese G2P, `cutlet.py` port, `ja_words.txt`), [hexgrad/misaki](https://github.com/hexgrad/misaki):
  Apache-2.0 (its LICENSE has no copyright line).
- **Kokoro ANE** generator, [mattmireles/kokoro-coreml @ `a3f1ff27`](https://github.com/mattmireles/kokoro-coreml/tree/a3f1ff27b1d8683efa9976704b46cb1d96da1a4b):
  Apache-2.0 (its LICENSE has no copyright line), modified by FluidInference (FluidAudio's `KokoroANE-v3/NOTICE`).
  Kokoro weights: [hexgrad/Kokoro-82M](https://huggingface.co/hexgrad/Kokoro-82M), Apache-2.0 (model card).
- **cutlet**, [polm/cutlet](https://github.com/polm/cutlet): MIT.

```text
MIT License

Copyright (c) 2020 Paul O'Leary McCann

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

- **num2kana**, [Greatdane/Convert-Numbers-to-Japanese](https://github.com/Greatdane/Convert-Numbers-to-Japanese): MIT.

```text
The MIT License (MIT)
Copyright (c) 2018 David Wilson, https://github.com/Greatdane

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in
all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
THE SOFTWARE.
```

- **unidic-lite**, [polm/unidic-lite](https://github.com/polm/unidic-lite): the package is MIT (`LICENSE`); the UniDic
  dictionary it redistributes is BSD-3-Clause (`LICENSE.unidic`).

```text
Copyright 2020 Paul McCann

Permission is hereby granted, free of charge, to any person obtaining a copy of
this software and associated documentation files (the "Software"), to deal in
the Software without restriction, including without limitation the rights to
use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies
of the Software, and to permit persons to whom the Software is furnished to do
so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

```text
Copyright (c) 2011-2017, The UniDic Consortium
All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are
met:

 * Redistributions of source code must retain the above copyright
   notice, this list of conditions and the following disclaimer.

 * Redistributions in binary form must reproduce the above copyright
   notice, this list of conditions and the following disclaimer in the
   documentation and/or other materials provided with the
   distribution.

 * Neither the name of the UniDic Consortium nor the names of its
   contributors may be used to endorse or promote products derived
   from this software without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS
"AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT
LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR
A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT
OWNER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT
LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE,
DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY
THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
(INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
```

- **ipa-dict** (es/fr G2P word lists), [open-dict-data/ipa-dict](https://github.com/open-dict-data/ipa-dict): MIT.

```text
The MIT License (MIT)

Copyright (c) 2016 dohliam

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

- **CharsiuG2P** (es/fr G2P fallback model), [lingjzhu/CharsiuG2P](https://github.com/lingjzhu/CharsiuG2P): MIT.

```text
MIT License

Copyright (c) 2022 jzhu

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

### fastcluster (BSD-2-Clause)

> Copyright: until package version 1.1.23: © 2011 Daniel Müllner <https://danifold.net>; all changes from version
> 1.1.24 on: © Google Inc. <https://www.google.com>. All rights reserved.
>
> Redistribution and use in source and binary forms, with or without modification, are permitted provided that the
> following conditions are met: (1) Redistributions of source code must retain the above copyright notice, this list
> of conditions and the following disclaimer. (2) Redistributions in binary form must reproduce the above copyright
> notice, this list of conditions and the following disclaimer in the documentation and/or other materials provided
> with the distribution.
>
> THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES,
> INCLUDING, BUT NOT LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
> DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
> SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
> SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY,
> WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
> OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

### text-processing-rs (NOTICE, Apache-2.0)

> text-processing-rs. Copyright 2026 FluidInference.
>
> This product is a Rust port of NVIDIA NeMo Text Processing (https://github.com/NVIDIA/NeMo-text-processing), which
> is licensed under the Apache License, Version 2.0. Copyright (c) NVIDIA CORPORATION & AFFILIATES. Licensed under the
> Apache License, Version 2.0.

### Spectacle (qrtz snap, ADR 0159)

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

## Rust crates in the app

Generated with `bun scripts/third-party-rust.mjs` (the normal-dependency closure of `qrtz-desktop` for
aarch64-apple-darwin from `cargo metadata --offline --locked`, without dev or build dependencies and without qrtz's
own crates). The `qrtz` command and the helpers' Rust hosts use a subset.
Counts per license: MIT OR Apache-2.0 (163), MIT (81), Apache-2.0 OR MIT (38), Unicode-3.0 (18), MIT/Apache-2.0 (13), Zlib OR Apache-2.0 OR MIT (12), Unlicense OR MIT (6), BSD-3-Clause (5), MPL-2.0 (5), MIT OR Apache-2.0 OR Zlib (4), Apache-2.0 (4), Zlib (2), Apache-2.0 OR ISC OR MIT (2), MIT OR Zlib OR Apache-2.0 (2), BSD-3-Clause OR Apache-2.0 (2), ISC (2), Unlicense/MIT (2), (MIT OR Apache-2.0) AND Unicode-3.0 (2), BSD-2-Clause OR Apache-2.0 OR MIT (2), 0BSD OR MIT OR Apache-2.0 (1), MIT-0 (1), BSD-3-Clause AND MIT (1), BSD-3-Clause/MIT (1), Apache-2.0/MIT (1), Apache-2.0 AND MIT (1), CC0-1.0 OR MIT-0 OR Apache-2.0 (1), Apache-2.0 / MIT (1), Apache-2.0 AND ISC (1), Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT (1), Apache-2.0 OR BSL-1.0 (1), CDLA-Permissive-2.0 (1).

| Crate                          | Version          | License                                             |
| ------------------------------ | ---------------- | --------------------------------------------------- |
| adler2                         | 2.0.1            | 0BSD OR MIT OR Apache-2.0                           |
| ahash                          | 0.8.12           | MIT OR Apache-2.0                                   |
| aho-corasick                   | 1.1.5            | Unlicense OR MIT                                    |
| alloc-no-stdlib                | 3.0.0            | BSD-3-Clause                                        |
| alloc-stdlib                   | 0.3.0            | BSD-3-Clause                                        |
| allocator-api2                 | 0.2.21           | MIT OR Apache-2.0                                   |
| anyhow                         | 1.0.104          | MIT OR Apache-2.0                                   |
| arboard                        | 3.7.0            | MIT OR Apache-2.0                                   |
| async-trait                    | 0.1.92           | MIT OR Apache-2.0                                   |
| atomic-waker                   | 1.1.2            | Apache-2.0 OR MIT                                   |
| auto-launch                    | 0.6.0            | MIT                                                 |
| base64                         | 0.21.7           | MIT OR Apache-2.0                                   |
| base64                         | 0.22.1           | MIT OR Apache-2.0                                   |
| base64                         | 0.23.1           | MIT OR Apache-2.0                                   |
| base64ct                       | 1.8.3            | Apache-2.0 OR MIT                                   |
| bit-set                        | 0.8.0            | Apache-2.0 OR MIT                                   |
| bit-vec                        | 0.8.0            | Apache-2.0 OR MIT                                   |
| bitflags                       | 1.3.2            | MIT/Apache-2.0                                      |
| bitflags                       | 2.13.2           | MIT OR Apache-2.0                                   |
| block-buffer                   | 0.10.4           | MIT OR Apache-2.0                                   |
| block2                         | 0.6.2            | MIT                                                 |
| borrow-or-share                | 0.2.4            | MIT-0                                               |
| brotli                         | 9.0.0            | BSD-3-Clause AND MIT                                |
| brotli-decompressor            | 6.0.1            | BSD-3-Clause/MIT                                    |
| bs58                           | 0.5.1            | MIT/Apache-2.0                                      |
| bumpalo                        | 3.20.3           | MIT OR Apache-2.0                                   |
| bytecount                      | 0.6.9            | Apache-2.0/MIT                                      |
| bytemuck                       | 1.25.2           | Zlib OR Apache-2.0 OR MIT                           |
| byteorder                      | 1.5.0            | Unlicense OR MIT                                    |
| byteorder-lite                 | 0.1.0            | Unlicense OR MIT                                    |
| bytes                          | 1.12.1           | MIT                                                 |
| camino                         | 1.2.6            | MIT OR Apache-2.0                                   |
| cargo_metadata                 | 0.19.2           | MIT                                                 |
| cargo-platform                 | 0.1.9            | MIT OR Apache-2.0                                   |
| cfb                            | 0.14.0           | MIT                                                 |
| cfg-if                         | 1.0.5            | MIT OR Apache-2.0                                   |
| chacha20                       | 0.10.2           | MIT OR Apache-2.0                                   |
| chrono                         | 0.4.45           | MIT OR Apache-2.0                                   |
| const-oid                      | 0.9.6            | Apache-2.0 OR MIT                                   |
| cookie                         | 0.18.2           | MIT OR Apache-2.0                                   |
| core-foundation                | 0.10.1           | MIT OR Apache-2.0                                   |
| core-foundation-sys            | 0.8.7            | MIT OR Apache-2.0                                   |
| core-graphics                  | 0.25.0           | MIT OR Apache-2.0                                   |
| core-graphics-types            | 0.2.0            | MIT OR Apache-2.0                                   |
| cpufeatures                    | 0.2.17           | MIT OR Apache-2.0                                   |
| crc32fast                      | 1.5.2            | MIT OR Apache-2.0                                   |
| crossbeam-channel              | 0.5.17           | MIT OR Apache-2.0                                   |
| crossbeam-utils                | 0.8.23           | MIT OR Apache-2.0                                   |
| crypto-common                  | 0.1.7            | MIT OR Apache-2.0                                   |
| cssparser                      | 0.37.0           | MPL-2.0                                             |
| cssparser-macros               | 0.7.1            | MPL-2.0                                             |
| ctor                           | 1.0.13           | Apache-2.0 OR MIT                                   |
| curve25519-dalek               | 4.1.3            | BSD-3-Clause                                        |
| darling                        | 0.24.1           | MIT                                                 |
| darling_core                   | 0.24.1           | MIT                                                 |
| darling_macro                  | 0.24.1           | MIT                                                 |
| data-encoding                  | 2.11.1           | MIT                                                 |
| defmt                          | 1.2.0            | MIT OR Apache-2.0                                   |
| defmt-macros                   | 1.2.0            | MIT OR Apache-2.0                                   |
| defmt-parser                   | 1.1.0            | MIT OR Apache-2.0                                   |
| der                            | 0.7.10           | Apache-2.0 OR MIT                                   |
| deranged                       | 0.5.9            | MIT OR Apache-2.0                                   |
| derive_more                    | 2.1.1            | MIT                                                 |
| derive_more-impl               | 2.1.1            | MIT                                                 |
| digest                         | 0.10.7           | MIT OR Apache-2.0                                   |
| dirs                           | 6.0.0            | MIT OR Apache-2.0                                   |
| dirs                           | 7.0.0            | MIT OR Apache-2.0                                   |
| dirs-sys                       | 0.5.0            | MIT OR Apache-2.0                                   |
| dispatch2                      | 0.3.1            | Zlib OR Apache-2.0 OR MIT                           |
| displaydoc                     | 0.2.7            | MIT OR Apache-2.0                                   |
| dom_query                      | 0.28.0           | MIT                                                 |
| dpi                            | 0.1.2            | Apache-2.0 AND MIT                                  |
| dtoa                           | 1.0.11           | MIT OR Apache-2.0                                   |
| dtoa-short                     | 0.3.5            | MPL-2.0                                             |
| dunce                          | 1.0.5            | CC0-1.0 OR MIT-0 OR Apache-2.0                      |
| dyn-clone                      | 1.0.20           | MIT OR Apache-2.0                                   |
| ed25519                        | 2.2.3            | Apache-2.0 OR MIT                                   |
| ed25519-dalek                  | 2.2.0            | BSD-3-Clause                                        |
| email_address                  | 0.2.9            | MIT                                                 |
| embed_plist                    | 1.2.2            | MIT OR Apache-2.0                                   |
| equivalent                     | 1.0.3            | Apache-2.0 OR MIT                                   |
| erased-serde                   | 0.4.10           | MIT OR Apache-2.0                                   |
| errno                          | 0.3.14           | MIT OR Apache-2.0                                   |
| fallible-iterator              | 0.3.0            | MIT/Apache-2.0                                      |
| fallible-streaming-iterator    | 0.1.9            | MIT/Apache-2.0                                      |
| fancy-regex                    | 0.19.2           | MIT                                                 |
| fastrand                       | 2.5.0            | Apache-2.0 OR MIT                                   |
| fax                            | 0.2.7            | MIT                                                 |
| fdeflate                       | 0.3.7            | MIT OR Apache-2.0                                   |
| flate2                         | 1.1.10           | MIT OR Apache-2.0                                   |
| fluent-uri                     | 0.4.1            | MIT                                                 |
| fnv                            | 1.0.7            | Apache-2.0 / MIT                                    |
| foldhash                       | 0.2.0            | Zlib                                                |
| foreign-types                  | 0.5.0            | MIT/Apache-2.0                                      |
| foreign-types-macros           | 0.2.4            | MIT/Apache-2.0                                      |
| foreign-types-shared           | 0.3.1            | MIT/Apache-2.0                                      |
| form_urlencoded                | 1.2.2            | MIT OR Apache-2.0                                   |
| fraction                       | 0.17.0           | MIT OR Apache-2.0                                   |
| futures-channel                | 0.3.34           | MIT OR Apache-2.0                                   |
| futures-core                   | 0.3.34           | MIT OR Apache-2.0                                   |
| futures-io                     | 0.3.34           | MIT OR Apache-2.0                                   |
| futures-macro                  | 0.3.34           | MIT OR Apache-2.0                                   |
| futures-sink                   | 0.3.34           | MIT OR Apache-2.0                                   |
| futures-task                   | 0.3.34           | MIT OR Apache-2.0                                   |
| futures-util                   | 0.3.34           | MIT OR Apache-2.0                                   |
| generic-array                  | 0.14.7           | MIT                                                 |
| getrandom                      | 0.2.17           | MIT OR Apache-2.0                                   |
| getrandom                      | 0.3.4            | MIT OR Apache-2.0                                   |
| getrandom                      | 0.4.3            | MIT OR Apache-2.0                                   |
| glob                           | 0.3.4            | MIT OR Apache-2.0                                   |
| global-hotkey                  | 0.8.0            | Apache-2.0 OR MIT                                   |
| half                           | 2.7.1            | MIT OR Apache-2.0                                   |
| hashbrown                      | 0.12.3           | MIT OR Apache-2.0                                   |
| hashbrown                      | 0.17.1           | MIT OR Apache-2.0                                   |
| hashlink                       | 0.12.2           | MIT OR Apache-2.0                                   |
| heck                           | 0.5.0            | MIT OR Apache-2.0                                   |
| hex                            | 0.4.3            | MIT OR Apache-2.0                                   |
| html5ever                      | 0.39.0           | MIT OR Apache-2.0                                   |
| http                           | 1.5.0            | MIT OR Apache-2.0                                   |
| http-body                      | 1.1.0            | MIT                                                 |
| http-body-util                 | 0.1.5            | MIT                                                 |
| httparse                       | 1.10.1           | MIT OR Apache-2.0                                   |
| hyper                          | 1.12.0           | MIT                                                 |
| hyper-rustls                   | 0.27.10          | Apache-2.0 OR ISC OR MIT                            |
| hyper-util                     | 0.1.21           | MIT                                                 |
| iana-time-zone                 | 0.1.65           | MIT OR Apache-2.0                                   |
| ico                            | 0.5.0            | MIT                                                 |
| icu_collections                | 2.1.1            | Unicode-3.0                                         |
| icu_locale_core                | 2.1.1            | Unicode-3.0                                         |
| icu_normalizer                 | 2.1.1            | Unicode-3.0                                         |
| icu_normalizer_data            | 2.1.1            | Unicode-3.0                                         |
| icu_properties                 | 2.1.2            | Unicode-3.0                                         |
| icu_properties_data            | 2.1.2            | Unicode-3.0                                         |
| icu_provider                   | 2.1.1            | Unicode-3.0                                         |
| ident_case                     | 1.0.1            | MIT/Apache-2.0                                      |
| idna                           | 1.1.0            | MIT OR Apache-2.0                                   |
| idna_adapter                   | 1.2.1            | Apache-2.0 OR MIT                                   |
| image                          | 0.25.10          | MIT OR Apache-2.0                                   |
| indexmap                       | 1.9.3            | Apache-2.0 OR MIT                                   |
| indexmap                       | 2.14.2           | Apache-2.0 OR MIT                                   |
| infer                          | 0.22.0           | MIT                                                 |
| ipnet                          | 2.12.2           | MIT OR Apache-2.0                                   |
| itoa                           | 1.0.18           | MIT OR Apache-2.0                                   |
| jiff                           | 0.2.38           | Unlicense OR MIT                                    |
| jiff-core                      | 0.1.1            | Unlicense OR MIT                                    |
| json-patch                     | 4.2.0            | MIT/Apache-2.0                                      |
| jsonptr                        | 0.7.1            | MIT OR Apache-2.0                                   |
| jsonschema                     | 0.59.0           | MIT                                                 |
| jsonschema-regex               | 0.59.0           | MIT                                                 |
| jsonschema-value               | 0.59.0           | MIT                                                 |
| keyboard-types                 | 0.7.0            | MIT OR Apache-2.0                                   |
| keyboard-types                 | 0.8.3            | MIT OR Apache-2.0                                   |
| lazy_static                    | 1.5.1            | MIT OR Apache-2.0                                   |
| libc                           | 0.2.191          | MIT OR Apache-2.0                                   |
| libsqlite3-sys                 | 0.38.2           | MIT                                                 |
| litemap                        | 0.8.3            | Unicode-3.0                                         |
| lock_api                       | 0.4.14           | MIT OR Apache-2.0                                   |
| log                            | 0.4.34           | MIT OR Apache-2.0                                   |
| lru-slab                       | 0.1.3            | MIT OR Apache-2.0 OR Zlib                           |
| mac-notification-sys           | 0.6.15           | MIT/Apache-2.0                                      |
| markup5ever                    | 0.39.0           | MIT OR Apache-2.0                                   |
| matchers                       | 0.2.0            | MIT                                                 |
| memchr                         | 2.8.3            | Unlicense OR MIT                                    |
| micromap                       | 0.3.0            | MIT                                                 |
| mime                           | 0.3.17           | MIT OR Apache-2.0                                   |
| minisign-verify                | 0.3.0            | MIT                                                 |
| miniz_oxide                    | 0.8.9            | MIT OR Zlib OR Apache-2.0                           |
| miniz_oxide                    | 0.9.1            | MIT OR Zlib OR Apache-2.0                           |
| mio                            | 1.2.4            | MIT                                                 |
| moxcms                         | 0.8.1            | BSD-3-Clause OR Apache-2.0                          |
| muda                           | 0.20.0           | Apache-2.0 OR MIT                                   |
| new_debug_unreachable          | 1.0.6            | MIT                                                 |
| nix                            | 0.31.3           | MIT                                                 |
| notify-rust                    | 4.18.2           | MIT OR Apache-2.0                                   |
| num                            | 0.4.3            | MIT OR Apache-2.0                                   |
| num-bigint                     | 0.4.8            | MIT OR Apache-2.0                                   |
| num-cmp                        | 0.1.0            | MIT/Apache-2.0                                      |
| num-complex                    | 0.4.6            | MIT OR Apache-2.0                                   |
| num-conv                       | 0.2.2            | MIT OR Apache-2.0                                   |
| num-integer                    | 0.1.47           | MIT OR Apache-2.0                                   |
| num-iter                       | 0.1.46           | MIT OR Apache-2.0                                   |
| num-rational                   | 0.4.2            | MIT OR Apache-2.0                                   |
| num-traits                     | 0.2.19           | MIT OR Apache-2.0                                   |
| objc2                          | 0.6.5            | MIT                                                 |
| objc2-app-kit                  | 0.3.2            | Zlib OR Apache-2.0 OR MIT                           |
| objc2-core-foundation          | 0.3.2            | Zlib OR Apache-2.0 OR MIT                           |
| objc2-core-graphics            | 0.3.2            | Zlib OR Apache-2.0 OR MIT                           |
| objc2-encode                   | 4.1.0            | MIT                                                 |
| objc2-exception-helper         | 0.1.1            | Zlib OR Apache-2.0 OR MIT                           |
| objc2-foundation               | 0.3.2            | MIT                                                 |
| objc2-io-surface               | 0.3.2            | Zlib OR Apache-2.0 OR MIT                           |
| objc2-quartz-core              | 0.3.2            | Zlib OR Apache-2.0 OR MIT                           |
| objc2-security                 | 0.3.2            | Zlib OR Apache-2.0 OR MIT                           |
| objc2-service-management       | 0.3.2            | Zlib OR Apache-2.0 OR MIT                           |
| objc2-web-kit                  | 0.3.2            | Zlib OR Apache-2.0 OR MIT                           |
| once_cell                      | 1.21.4           | MIT OR Apache-2.0                                   |
| option-ext                     | 0.2.0            | MPL-2.0                                             |
| os_info                        | 3.15.0           | MIT                                                 |
| outref                         | 0.5.2            | MIT                                                 |
| parking_lot                    | 0.12.5           | MIT OR Apache-2.0                                   |
| parking_lot_core               | 0.9.12           | MIT OR Apache-2.0                                   |
| percent-encoding               | 2.3.2            | MIT OR Apache-2.0                                   |
| phf                            | 0.13.1           | MIT                                                 |
| phf_generator                  | 0.13.1           | MIT                                                 |
| phf_macros                     | 0.13.1           | MIT                                                 |
| phf_shared                     | 0.13.1           | MIT                                                 |
| pin-project-lite               | 0.2.17           | Apache-2.0 OR MIT                                   |
| pkcs8                          | 0.10.2           | Apache-2.0 OR MIT                                   |
| plist                          | 1.10.1           | MIT                                                 |
| png                            | 0.17.16          | MIT OR Apache-2.0                                   |
| png                            | 0.18.1           | MIT OR Apache-2.0                                   |
| potential_utf                  | 0.1.6            | Unicode-3.0                                         |
| powerfmt                       | 0.2.1            | MIT OR Apache-2.0                                   |
| ppv-lite86                     | 0.2.21           | MIT OR Apache-2.0                                   |
| precomputed-hash               | 0.1.1            | MIT                                                 |
| proc-macro2                    | 1.0.107          | MIT OR Apache-2.0                                   |
| pxfm                           | 0.1.30           | BSD-3-Clause OR Apache-2.0                          |
| quick-error                    | 2.0.1            | MIT/Apache-2.0                                      |
| quick-xml                      | 0.42.0           | MIT                                                 |
| quinn                          | 0.11.12          | MIT OR Apache-2.0                                   |
| quinn-proto                    | 0.11.19          | MIT OR Apache-2.0                                   |
| quinn-udp                      | 0.5.16           | MIT OR Apache-2.0                                   |
| quote                          | 1.0.47           | MIT OR Apache-2.0                                   |
| rand                           | 0.10.3           | MIT OR Apache-2.0                                   |
| rand                           | 0.9.5            | MIT OR Apache-2.0                                   |
| rand_chacha                    | 0.9.0            | MIT OR Apache-2.0                                   |
| rand_core                      | 0.10.1           | MIT OR Apache-2.0                                   |
| rand_core                      | 0.6.4            | MIT OR Apache-2.0                                   |
| rand_core                      | 0.9.5            | MIT OR Apache-2.0                                   |
| rand_pcg                       | 0.10.2           | MIT OR Apache-2.0                                   |
| raw-window-handle              | 0.6.2            | MIT OR Apache-2.0 OR Zlib                           |
| ref-cast                       | 1.0.27           | MIT OR Apache-2.0                                   |
| ref-cast-impl                  | 1.0.27           | MIT OR Apache-2.0                                   |
| referencing                    | 0.59.0           | MIT                                                 |
| regex                          | 1.13.1           | MIT OR Apache-2.0                                   |
| regex-automata                 | 0.4.18           | MIT OR Apache-2.0                                   |
| regex-syntax                   | 0.8.11           | MIT OR Apache-2.0                                   |
| reqwest                        | 0.12.28          | MIT OR Apache-2.0                                   |
| rfd                            | 0.16.0           | MIT                                                 |
| ring                           | 0.17.14          | Apache-2.0 AND ISC                                  |
| rusqlite                       | 0.40.2           | MIT                                                 |
| rustc-hash                     | 2.1.3            | Apache-2.0 OR MIT                                   |
| rustix                         | 1.1.5            | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| rustls                         | 0.23.46          | Apache-2.0 OR ISC OR MIT                            |
| rustls-pki-types               | 1.15.1           | MIT OR Apache-2.0                                   |
| rustls-webpki                  | 0.103.15         | ISC                                                 |
| ryu                            | 1.0.23           | Apache-2.0 OR BSL-1.0                               |
| same-file                      | 1.0.6            | Unlicense/MIT                                       |
| schemars                       | 0.8.22           | MIT                                                 |
| schemars                       | 0.9.0            | MIT                                                 |
| schemars                       | 1.2.3            | MIT                                                 |
| schemars_derive                | 0.8.22           | MIT                                                 |
| scopeguard                     | 1.2.0            | MIT OR Apache-2.0                                   |
| security-framework             | 3.7.0            | MIT OR Apache-2.0                                   |
| security-framework-sys         | 2.17.0           | MIT OR Apache-2.0                                   |
| selectors                      | 0.38.0           | MPL-2.0                                             |
| semver                         | 1.0.28           | MIT OR Apache-2.0                                   |
| serde                          | 1.0.229          | MIT OR Apache-2.0                                   |
| serde_core                     | 1.0.229          | MIT OR Apache-2.0                                   |
| serde_derive                   | 1.0.229          | MIT OR Apache-2.0                                   |
| serde_derive_internals         | 0.29.1           | MIT OR Apache-2.0                                   |
| serde_json                     | 1.0.154          | MIT OR Apache-2.0                                   |
| serde_repr                     | 0.1.21           | MIT OR Apache-2.0                                   |
| serde_spanned                  | 1.1.2            | MIT OR Apache-2.0                                   |
| serde_urlencoded               | 0.7.1            | MIT/Apache-2.0                                      |
| serde_with                     | 3.24.0           | MIT OR Apache-2.0                                   |
| serde_with_macros              | 3.24.0           | MIT OR Apache-2.0                                   |
| serde-untagged                 | 0.1.9            | MIT OR Apache-2.0                                   |
| serialize-to-javascript        | 0.1.2            | MIT OR Apache-2.0                                   |
| serialize-to-javascript-impl   | 0.1.2            | MIT OR Apache-2.0                                   |
| servo_arc                      | 0.4.3            | MIT OR Apache-2.0                                   |
| sha2                           | 0.10.9           | MIT OR Apache-2.0                                   |
| sharded-slab                   | 0.1.7            | MIT                                                 |
| signal-hook-registry           | 1.4.8            | MIT OR Apache-2.0                                   |
| signature                      | 2.2.0            | Apache-2.0 OR MIT                                   |
| simd-adler32                   | 0.3.10           | MIT                                                 |
| siphasher                      | 1.0.4            | MIT OR Apache-2.0                                   |
| slab                           | 0.4.12           | MIT                                                 |
| smallvec                       | 1.16.3           | MIT OR Apache-2.0                                   |
| smappservice-rs                | 0.1.3            | MIT                                                 |
| socket2                        | 0.6.5            | MIT OR Apache-2.0                                   |
| spki                           | 0.7.3            | Apache-2.0 OR MIT                                   |
| stable_deref_trait             | 1.2.1            | MIT OR Apache-2.0                                   |
| string_cache                   | 0.9.0            | MIT OR Apache-2.0                                   |
| strsim                         | 0.11.1           | MIT                                                 |
| strum                          | 0.28.0           | MIT                                                 |
| strum_macros                   | 0.28.0           | MIT                                                 |
| subtle                         | 2.6.1            | BSD-3-Clause                                        |
| swift-rs                       | 1.0.9            | MIT OR Apache-2.0                                   |
| syn                            | 2.0.119          | MIT OR Apache-2.0                                   |
| syn                            | 3.0.9            | MIT OR Apache-2.0                                   |
| sync_wrapper                   | 1.0.2            | Apache-2.0                                          |
| synstructure                   | 0.14.0           | MIT                                                 |
| tao                            | 0.37.1           | Apache-2.0                                          |
| tauri                          | 2.12.4           | Apache-2.0 OR MIT                                   |
| tauri-codegen                  | 2.7.3            | Apache-2.0 OR MIT                                   |
| tauri-macros                   | 2.7.3            | Apache-2.0 OR MIT                                   |
| tauri-plugin-autostart         | 2.7.0            | Apache-2.0 OR MIT                                   |
| tauri-plugin-clipboard-manager | 2.5.0            | Apache-2.0 OR MIT                                   |
| tauri-plugin-dialog            | 2.8.3            | Apache-2.0 OR MIT                                   |
| tauri-plugin-fs                | 2.7.1            | Apache-2.0 OR MIT                                   |
| tauri-plugin-global-shortcut   | 2.4.1            | Apache-2.0 OR MIT                                   |
| tauri-plugin-notification      | 2.7.0            | Apache-2.0 OR MIT                                   |
| tauri-plugin-single-instance   | 2.5.4            | Apache-2.0 OR MIT                                   |
| tauri-runtime                  | 2.12.2           | Apache-2.0 OR MIT                                   |
| tauri-runtime-wry              | 2.12.2           | Apache-2.0 OR MIT                                   |
| tauri-utils                    | 2.10.2           | Apache-2.0 OR MIT                                   |
| tempfile                       | 3.27.0           | MIT OR Apache-2.0                                   |
| tendril                        | 0.5.1            | MIT OR Apache-2.0                                   |
| thiserror                      | 2.0.21           | MIT OR Apache-2.0                                   |
| thiserror-impl                 | 2.0.21           | MIT OR Apache-2.0                                   |
| thread_local                   | 1.1.10           | MIT OR Apache-2.0                                   |
| tiff                           | 0.11.3           | MIT                                                 |
| time                           | 0.3.55           | MIT OR Apache-2.0                                   |
| time-core                      | 0.1.9            | MIT OR Apache-2.0                                   |
| time-macros                    | 0.2.32           | MIT OR Apache-2.0                                   |
| tinystr                        | 0.8.4            | Unicode-3.0                                         |
| tinyvec                        | 1.13.3           | Zlib OR Apache-2.0 OR MIT                           |
| tokio                          | 1.53.3           | MIT                                                 |
| tokio-macros                   | 2.7.2            | MIT                                                 |
| tokio-rustls                   | 0.26.6           | MIT OR Apache-2.0                                   |
| tokio-util                     | 0.7.20           | MIT                                                 |
| toml                           | 1.1.8+spec-1.1.0 | MIT OR Apache-2.0                                   |
| toml_datetime                  | 1.1.2+spec-1.1.0 | MIT OR Apache-2.0                                   |
| toml_parser                    | 1.1.5+spec-1.1.0 | MIT OR Apache-2.0                                   |
| toml_writer                    | 1.1.3+spec-1.1.0 | MIT OR Apache-2.0                                   |
| tower                          | 0.5.3            | MIT                                                 |
| tower-http                     | 0.6.11           | MIT                                                 |
| tower-layer                    | 0.3.3            | MIT                                                 |
| tower-service                  | 0.3.3            | MIT                                                 |
| tracing                        | 0.1.44           | MIT                                                 |
| tracing-attributes             | 0.1.31           | MIT                                                 |
| tracing-core                   | 0.1.36           | MIT                                                 |
| tracing-log                    | 0.2.0            | MIT                                                 |
| tracing-subscriber             | 0.3.23           | MIT                                                 |
| tray-icon                      | 0.25.1           | MIT OR Apache-2.0                                   |
| try-lock                       | 0.2.5            | MIT                                                 |
| typed-path                     | 0.12.3           | MIT OR Apache-2.0                                   |
| typeid                         | 1.0.3            | MIT OR Apache-2.0                                   |
| typenum                        | 1.20.1           | MIT OR Apache-2.0                                   |
| unicode-general-category       | 1.1.0            | Apache-2.0                                          |
| unicode-ident                  | 1.0.27           | (MIT OR Apache-2.0) AND Unicode-3.0                 |
| unicode-normalization          | 0.1.26           | (MIT OR Apache-2.0) AND Unicode-3.0                 |
| unicode-segmentation           | 1.13.3           | MIT OR Apache-2.0                                   |
| untrusted                      | 0.9.0            | ISC                                                 |
| url                            | 2.5.8            | MIT OR Apache-2.0                                   |
| urlpattern                     | 0.6.0            | MIT                                                 |
| utf8_iter                      | 1.0.4            | Apache-2.0 OR MIT                                   |
| uuid                           | 1.26.1           | Apache-2.0 OR MIT                                   |
| uuid-simd                      | 0.8.0            | MIT                                                 |
| vsimd                          | 0.8.0            | MIT                                                 |
| walkdir                        | 2.5.0            | Unlicense/MIT                                       |
| want                           | 0.3.2            | MIT                                                 |
| web_atoms                      | 0.2.6            | MIT OR Apache-2.0                                   |
| web-time                       | 1.1.0            | MIT OR Apache-2.0                                   |
| webpki-roots                   | 1.0.9            | CDLA-Permissive-2.0                                 |
| weezl                          | 0.1.12           | MIT OR Apache-2.0                                   |
| window-vibrancy                | 0.8.1            | Apache-2.0 OR MIT                                   |
| winnow                         | 1.0.4            | MIT                                                 |
| writeable                      | 0.6.4            | Unicode-3.0                                         |
| wry                            | 0.57.0           | Apache-2.0 OR MIT                                   |
| yoke                           | 0.8.3            | Unicode-3.0                                         |
| yoke-derive                    | 0.8.4            | Unicode-3.0                                         |
| zerocopy                       | 0.8.63           | BSD-2-Clause OR Apache-2.0 OR MIT                   |
| zerocopy-derive                | 0.8.63           | BSD-2-Clause OR Apache-2.0 OR MIT                   |
| zerofrom                       | 0.1.8            | Unicode-3.0                                         |
| zerofrom-derive                | 0.1.8            | Unicode-3.0                                         |
| zeroize                        | 1.9.1            | Apache-2.0 OR MIT                                   |
| zerotrie                       | 0.2.5            | Unicode-3.0                                         |
| zerovec                        | 0.11.8           | Unicode-3.0                                         |
| zerovec-derive                 | 0.11.6           | Unicode-3.0                                         |
| zip                            | 9.0.5            | MIT                                                 |
| zlib-rs                        | 0.6.8            | Zlib                                                |
| zmij                           | 1.0.23           | MIT                                                 |
| zopfli                         | 0.8.4            | Apache-2.0                                          |
| zune-core                      | 0.5.3            | MIT OR Apache-2.0 OR Zlib                           |
| zune-jpeg                      | 0.5.15           | MIT OR Apache-2.0 OR Zlib                           |

## JavaScript production dependencies

Generated with `bun scripts/third-party.mjs` (the `dependencies` tree of `apps/desktop`, without dev dependencies).
Vite bundles it into the app's renderer.

| @tauri-apps/api | 2.12.4 | Apache-2.0 OR MIT |
| cookie | 1.1.1 | MIT |
| js-tokens | 4.0.0 | MIT |
| loose-envify | 1.4.0 | MIT |
| object-assign | 4.1.1 | MIT |
| prop-types | 15.8.1 | MIT |
| react | 19.3.1 | MIT |
| react-dom | 19.3.1 | MIT |
| react-feather | 2.0.10 | MIT |
| react-is | 16.13.1 | MIT |
| react-router | 7.18.4 | MIT |
| scheduler | 0.28.0 | MIT |
| set-cookie-parser | 2.7.2 | MIT |
| zod | 4.6.7 | MIT |
