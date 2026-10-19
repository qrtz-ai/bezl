import { z } from 'zod';
import { SnapStatus } from '../snap/contract';
import type { Capability } from '../capabilities';
import { CalendarStatus, HelperStatus } from './contract';
import { PermissionsStatus, SessionView } from '../note/contract';
import { LiveTranscriptDelta, PipelineView } from '../note/lifecycle';
import { TalkView } from '../talk/contract';
import { SpeechSetupStatus } from '../speech/setup';
import { PackagesProgress } from '../packages/contract';
import { AppUpdateStatus } from '../app-update/contract';

export interface EventDef<P extends z.ZodType = z.ZodType> {
  capability: Capability;
  payload: P;
  /**
   * High-rate (several a second while recording, downloading or backing up): delivered to each window over its
   * own Tauri `Channel` stream instead of the broadcast event (ADR 0095).
   */
  stream?: true;
}
const defineEvent = <P extends z.ZodType>(d: EventDef<P>) => d;

/** Main → renderer push events. */
export const eventContract = {
  'helper:status': defineEvent({
    capability: 'helper.status',
    payload: HelperStatus,
  }),
  /** ADR 0217: nano's calendar status changed (a read, access, on/off). */
  'calendar:changed': defineEvent({
    capability: 'calendar.read',
    payload: CalendarStatus,
  }),
  'config:changed': defineEvent({
    capability: 'config.read',
    payload: z.record(z.string(), z.unknown()),
  }),
  /** qrtz snap (ADR 0159): qrtz-snap reported a new status (started, stopped, re-registered its shortcuts). */
  'snap:changed': defineEvent({
    capability: 'snap.read',
    payload: SnapStatus,
  }),
  'note:session': defineEvent({
    capability: 'note.read',
    payload: SessionView,
    stream: true,
  }),
  'note:pipeline': defineEvent({
    capability: 'note.read',
    payload: PipelineView,
  }),
  'note:transcript': defineEvent({
    capability: 'note.read',
    payload: LiveTranscriptDelta,
    stream: true,
  }),
  /** ADR 0220: notes changed on disk or through qrtz (ids of changed or removed notes). */
  'doc:changed': defineEvent({
    capability: 'doc.read',
    payload: z.object({ ids: z.array(z.string()).max(1000) }),
  }),
  /** The recorded meeting ended: the recording stops at `deadlineAt` unless it comes back or is kept. */
  'note:auto-stop': defineEvent({
    capability: 'note.read',
    payload: z.discriminatedUnion('state', [
      z.object({ state: z.literal('armed'), deadlineAt: z.number() }),
      z.object({ state: z.literal('cleared') }),
    ]),
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
    stream: true,
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
    stream: true,
  }),
  /** ADR 0161: the marketplace's install bar while `packages:apply` downloads. */
  'packages:progress': defineEvent({
    capability: 'packages.read',
    payload: PackagesProgress,
    stream: true,
  }),
  /** ADR 0181: qrtz's own update state changed (checking, available, download progress, ready). */
  'app-update:state': defineEvent({
    capability: 'app.update.read',
    payload: AppUpdateStatus,
  }),
  /** A ctrl do run changed (started, asked, acted, stopped); the view reloads without polling. */
  'ctrl:changed': defineEvent({
    capability: 'ctrl.read',
    payload: z.object({ ids: z.array(z.string()) }),
  }),
} as const satisfies Record<string, EventDef>;

export type EventContract = typeof eventContract;
export type EventName = keyof EventContract;
export type EventPayload<E extends EventName> = z.output<
  EventContract[E]['payload']
>;

/** Events delivered over streams (ADR 0095). */
export const isStreamEvent = (e: string): boolean =>
  isEventName(e) && (eventContract[e] as EventDef).stream === true;

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
