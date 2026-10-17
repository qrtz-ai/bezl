import Module, { createRequire } from 'node:module';
import path from 'node:path';
import type { Embedder } from '@qrtz/core';

/** all-MiniLM-L6-v2 (q8, ~23 MB, 384-d): small enough for CPU in the worker; downloaded once into userData/models. */
export const EMBED_MODEL = 'Xenova/all-MiniLM-L6-v2';

/**
 * Lazily loads transformers.js and the embedding model on first use. Returns null when disabled
 * (QRTZ_EMBEDDINGS=0); throws when the library/model can't load, which the handlers turn into the
 * full-text-only fallback.
 */
export type EmbedBackend = 'native' | 'wasm';

const req: NodeJS.Require =
  typeof require === 'function' ? require : createRequire(import.meta.url);

/** Native onnxruntime-node loads only where a prebuilt binary exists (not darwin-x64 in 1.30). */
function nativeAvailable(): boolean {
  try {
    req('onnxruntime-node');
    return true;
  } catch {
    return false;
  }
}

/**
 * Picks the ONNX backend. `QRTZ_EMBEDDINGS_BACKEND=native|wasm` forces one; the default tries native,
 * then onnxruntime-web (WASM, single-threaded CPU). Both produce the same vectors, so the index is shared.
 */
export function chooseBackend(
  forced = process.env.QRTZ_EMBEDDINGS_BACKEND,
  native = nativeAvailable,
): EmbedBackend {
  if (forced === 'wasm') return 'wasm';
  if (forced === 'native') return 'native';
  return native() ? 'native' : 'wasm';
}

let wasmAliased = false;
/** transformers.js hard-requires 'onnxruntime-node'; point that at onnxruntime-web's Node build (same API). */
function useWasmRuntime(): void {
  if (wasmAliased) return;
  const ortWeb = req('onnxruntime-web') as {
    env: { wasm: { numThreads: number; wasmPaths: string } };
  };
  // No worker threads (blob: URLs aren't loadable in Node); load the .wasm from disk (asar-unpacked when packaged).
  ortWeb.env.wasm.numThreads = 1;
  ortWeb.env.wasm.wasmPaths =
    path
      .dirname(req.resolve('onnxruntime-web'))
      .replace(`app.asar${path.sep}`, `app.asar.unpacked${path.sep}`) +
    path.sep;
  const M = Module as unknown as {
    _resolveFilename: (r: string, ...rest: unknown[]) => string;
  };
  const orig = M._resolveFilename;
  M._resolveFilename = function (r: string, ...rest: unknown[]) {
    return orig.call(
      this,
      r === 'onnxruntime-node' ? 'onnxruntime-web' : r,
      ...rest,
    );
  };
  wasmAliased = true;
}

export function transformersEmbedder(
  modelsDir: string,
): () => Promise<(Embedder & { backend: EmbedBackend }) | null> {
  let p: Promise<Embedder & { backend: EmbedBackend }> | null = null;
  return async () => {
    if (process.env.QRTZ_EMBEDDINGS === '0') return null;
    p ??= (async () => {
      const backend = chooseBackend();
      if (backend === 'wasm') useWasmRuntime();
      const { pipeline, env } = await import('@huggingface/transformers');
      env.cacheDir = path.join(modelsDir, 'transformers');
      env.allowRemoteModels = process.env.QRTZ_EMBEDDINGS_OFFLINE !== '1';
      const fe = await pipeline('feature-extraction', EMBED_MODEL, {
        dtype: 'q8',
      });
      return {
        id: `${EMBED_MODEL}@q8`,
        backend,
        async embed(texts: string[]) {
          if (!texts.length) return [];
          const t = await fe(texts, { pooling: 'mean', normalize: true });
          const [n, d] = t.dims as [number, number];
          const data = t.data as Float32Array;
          return Array.from({ length: n }, (_, i) =>
            data.slice(i * d, (i + 1) * d),
          );
        },
      };
    })();
    try {
      return await p;
    } catch (e) {
      p = null;
      throw e;
    }
  };
}
