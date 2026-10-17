import { z } from 'zod';
import type { Capability } from '../capabilities';
import { HelperStatus } from './contract';

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
