import path from 'node:path';
import type { Embedder } from '@qrtz/core';

/** all-MiniLM-L6-v2 (q8, ~23 MB, 384-d): small enough for CPU in the worker; downloaded once into userData/models. */
export const EMBED_MODEL = 'Xenova/all-MiniLM-L6-v2';

/**
 * Lazily loads transformers.js and the embedding model on first use. Returns null when disabled
 * (QRTZ_EMBEDDINGS=0); throws when the library/model can't load, which the handlers turn into the
 * full-text-only fallback.
 */
export function transformersEmbedder(
  modelsDir: string,
): () => Promise<Embedder | null> {
  let p: Promise<Embedder> | null = null;
  return async () => {
    if (process.env.QRTZ_EMBEDDINGS === '0') return null;
    p ??= (async () => {
      const { pipeline, env } = await import('@huggingface/transformers');
      env.cacheDir = path.join(modelsDir, 'transformers');
      env.allowRemoteModels = process.env.QRTZ_EMBEDDINGS_OFFLINE !== '1';
      const fe = await pipeline('feature-extraction', EMBED_MODEL, {
        dtype: 'q8',
      });
      return {
        id: `${EMBED_MODEL}@q8`,
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
