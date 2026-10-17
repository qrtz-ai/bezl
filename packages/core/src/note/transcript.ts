import type { AsrEvent, Segment } from './types';

/**
 * Folds a streaming ASR event sequence into finalized segments + live partials.
 * Partials for an utterance are replaced; a final closes it. Out-of-order finals are sorted.
 */
export class TranscriptAssembler {
  private finals = new Map<string, Segment>();
  private partials = new Map<string, { startMs: number; text: string }>();

  push(e: AsrEvent): void {
    if (e.kind === 'partial') {
      if (this.finals.has(e.utteranceId)) return; // late partial after final
      this.partials.set(e.utteranceId, { startMs: e.startMs, text: e.text });
      return;
    }
    this.partials.delete(e.utteranceId);
    const text = e.text.trim();
    if (!text) return void this.finals.delete(e.utteranceId);
    this.finals.set(e.utteranceId, {
      id: e.utteranceId,
      startMs: e.startMs,
      endMs: Math.max(e.endMs, e.startMs),
      text,
      source: 'live',
      ...(e.confidence === undefined ? {} : { confidence: e.confidence }),
    });
  }

  segments(): Segment[] {
    return [...this.finals.values()].sort((a, b) => a.startMs - b.startMs);
  }

  /** Current in-flight hypotheses, oldest first (for the HUD). */
  livePartials(): { utteranceId: string; startMs: number; text: string }[] {
    return [...this.partials.entries()]
      .map(([utteranceId, p]) => ({ utteranceId, ...p }))
      .sort((a, b) => a.startMs - b.startMs);
  }

  /** Last N characters of finals + partials, for a compact live caption line. */
  tail(maxChars = 240): string {
    const s = [
      ...this.segments().map((x) => x.text),
      ...this.livePartials().map((p) => p.text),
    ].join(' ');
    return s.length <= maxChars ? s : '…' + s.slice(s.length - maxChars + 1);
  }
}
