import type { ChatSource } from './chat';
import { rankSources } from './chat';

/** On-device sentence embedder (worker: transformers.js MiniLM). Vectors must be L2-normalized. */
export interface Embedder {
  readonly id: string;
  embed(texts: string[]): Promise<Float32Array[]>;
}

export const vecToBlob = (v: Float32Array) =>
  new Uint8Array(v.buffer, v.byteOffset, v.byteLength);
export const blobToVec = (b: Uint8Array) =>
  new Float32Array(
    b.buffer.slice(b.byteOffset, b.byteOffset + b.byteLength) as ArrayBuffer,
  );

export function dot(a: Float32Array, b: Float32Array): number {
  let s = 0;
  const n = Math.min(a.length, b.length);
  for (let i = 0; i < n; i++) s += a[i]! * b[i]!;
  return s;
}

export type VecSource = ChatSource & { vec?: Float32Array };

/**
 * Hybrid retrieval: reciprocal-rank fusion of lexical ranking (term overlap) and cosine similarity to the
 * question embedding. Without a question vector it is just the lexical ranking (the fallback).
 */
export function hybridRank(
  question: string,
  candidates: VecSource[],
  qvec: Float32Array | null,
  k = 24,
  minSim = 0.25,
): ChatSource[] {
  const strip = ({ vec: _v, ...s }: VecSource): ChatSource => s;
  if (!qvec) return rankSources(question, candidates.map(strip), k);
  const K = 60;
  const score = new Map<VecSource, number>();
  rankSources(question, candidates, candidates.length).forEach((s, i) =>
    score.set(s as VecSource, (score.get(s as VecSource) ?? 0) + 1 / (K + i)),
  );
  candidates
    .filter((c) => c.vec)
    .map((c) => ({ c, sim: dot(qvec, c.vec!) }))
    .filter((x) => x.sim >= minSim)
    .sort((a, b) => b.sim - a.sim)
    .forEach(({ c }, i) => score.set(c, (score.get(c) ?? 0) + 1 / (K + i)));
  return [...score.entries()]
    .sort((a, b) => b[1] - a[1])
    .slice(0, k)
    .map(([c]) => strip(c));
}
