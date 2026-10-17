import fs from 'node:fs';
import path from 'node:path';

/**
 * Deferred (undoable) meeting deletes, made durable with a tombstone file. A tombstone is written the moment
 * the user confirms; the real delete runs when the undo window ends. If the app quits or crashes first, the
 * tombstone is completed on the next launch. Lists hide tombstoned meetings in the meantime.
 */
export class PendingDeletes {
  private timers = new Map<string, ReturnType<typeof setTimeout>>();
  private file: string;
  constructor(
    dir: string,
    private readonly finalize: (id: string) => Promise<void>,
    private readonly onError: (id: string, e: unknown) => void = () => {},
  ) {
    this.file = path.join(dir, 'pending-deletes.json');
  }

  ids(): string[] {
    try {
      const v = JSON.parse(fs.readFileSync(this.file, 'utf8')) as unknown;
      return Array.isArray(v)
        ? v.filter((x): x is string => typeof x === 'string')
        : [];
    } catch {
      return [];
    }
  }
  has(id: string) {
    return this.ids().includes(id);
  }
  private write(ids: string[]) {
    const tmp = `${this.file}.tmp`;
    fs.mkdirSync(path.dirname(this.file), { recursive: true });
    fs.writeFileSync(tmp, JSON.stringify([...new Set(ids)]));
    fs.renameSync(tmp, this.file); // atomic: a crash never leaves a half-written tombstone file
  }

  schedule(id: string, deferMs: number) {
    this.write([...this.ids(), id]);
    clearTimeout(this.timers.get(id));
    this.timers.set(
      id,
      setTimeout(() => void this.complete(id), deferMs),
    );
  }
  /**
   * Returns false when it is too late: the window ended, so the delete is running or already went through (or
   * failed and waits for the next launch), or the tombstone came from an earlier launch. Only a delete still
   * waiting out its window is undone; `true` for one that is already running would hide a meeting that is being
   * removed (N51).
   */
  undo(id: string): boolean {
    const t = this.timers.get(id);
    if (t === undefined) return false;
    clearTimeout(t);
    this.timers.delete(id);
    this.write(this.ids().filter((x) => x !== id));
    return true;
  }
  async complete(id: string) {
    this.timers.delete(id);
    try {
      await this.finalize(id);
      this.write(this.ids().filter((x) => x !== id));
    } catch (e) {
      this.onError(id, e); // tombstone stays; retried on next launch
    }
  }
  /** On launch: finish every delete a quit or crash interrupted. */
  async recover() {
    for (const id of this.ids()) await this.complete(id);
  }
}
