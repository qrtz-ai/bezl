import { z } from 'zod';
import type { Capability } from '../capabilities';
import { HelperStatus } from './contract';
import { PermissionsStatus, SessionView } from '../note/contract';
import { LiveTranscriptDelta, PipelineView } from '../note/lifecycle';
import { TalkView } from '../talk/contract';
import { SearchModelStatus, SpeechSetupStatus } from '../speech/setup';

export interface EventDef<P extends z.ZodType = z.ZodType> {
  capability: Capability;
  payload: P;
}
const defineEvent = <P extends z.ZodType>(d: EventDef<P>) => d;

/** Main → renderer push events. */
export const eventContract = {
  'helper:status': defineEvent({
    capability: 'helper.status',
    payload: HelperStatus,
  }),
  'config:changed': defineEvent({
    capability: 'config.read',
    payload: z.record(z.string(), z.unknown()),
  }),
  'note:session': defineEvent({
    capability: 'note.read',
    payload: SessionView,
  }),
  'note:pipeline': defineEvent({
    capability: 'note.read',
    payload: PipelineView,
  }),
  'note:transcript': defineEvent({
    capability: 'note.read',
    payload: LiveTranscriptDelta,
  }),
  'note:changed': defineEvent({
    capability: 'note.read',
    payload: z.object({ id: z.string() }),
  }),
  'data:backup-progress': defineEvent({
    capability: 'data.backup',
    payload: z.object({
      kind: z.enum(['export', 'import']),
      phase: z.enum(['checksum', 'write', 'verify', 'import', 'done']),
      done: z.number(),
      total: z.number(),
    }),
  }),
  'talk:state': defineEvent({ capability: 'talk.read', payload: TalkView }),
  'talk:history-changed': defineEvent({
    capability: 'talk.read',
    payload: z.object({ id: z.string() }),
  }),
  'permissions:changed': defineEvent({
    capability: 'os.permissions',
    payload: PermissionsStatus,
  }),
  'speech:status': defineEvent({
    capability: 'speech.read',
    payload: SpeechSetupStatus,
  }),
  'search:model-status': defineEvent({
    capability: 'speech.read',
    payload: SearchModelStatus,
  }),
} as const satisfies Record<string, EventDef>;

export type EventContract = typeof eventContract;
export type EventName = keyof EventContract;
export type EventPayload<E extends EventName> = z.output<
  EventContract[E]['payload']
>;

export const isEventName = (e: unknown): e is EventName =>
  typeof e === 'string' &&
  Object.prototype.hasOwnProperty.call(eventContract, e);

type Listener<E extends EventName> = (payload: EventPayload<E>) => void;

/** In-process typed bus. Payloads are validated on emit so bad data never fans out. */
export class EventBus {
  private listeners = new Map<EventName, Set<Listener<EventName>>>();

  on<E extends EventName>(event: E, fn: Listener<E>): () => void {
    let set = this.listeners.get(event);
    if (!set) this.listeners.set(event, (set = new Set()));
    set.add(fn as Listener<EventName>);
    return () => set.delete(fn as Listener<EventName>);
  }

  emit<E extends EventName>(event: E, payload: EventPayload<E>): void {
    const parsed = eventContract[event].payload.parse(
      payload,
    ) as EventPayload<E>;
    for (const fn of this.listeners.get(event) ?? []) {
      try {
        fn(parsed);
      } catch {
        // A faulty listener must not break other listeners.
      }
    }
  }
}
