import { useCallback, useEffect, useState } from 'react';
import {
  Button,
  Icon,
  SettingRow,
  SettingsSection,
  Stack,
  Switch,
  Text,
} from '@qrtz/ui';
import type { AppError, CalendarEvent, CalendarStatus } from '@qrtz/core';
import { useQrtz, useQrtzEvent } from '@qrtz/core/react';

/** ADR 0217: what the access state means to the user (lowercase, no jargon). */
export function accessNote(s: CalendarStatus | null): string | null {
  if (!s) return null;
  switch (s.access) {
    case 'granted':
      return null;
    case 'not-determined':
      return 'qrtz will ask macos for calendar access.';
    case 'unsupported':
      return 'the calendar works on a mac only.';
    case 'write-only':
      return 'qrtz can only add events, not read them. allow full access in system settings › privacy & security › calendars.';
    default:
      return 'calendar access is off. allow it in system settings › privacy & security › calendars.';
  }
}

/** The calendar's status and the upcoming events, live on `calendar:changed`. */
export function useCalendar() {
  const qrtz = useQrtz();
  const [status, setStatus] = useState<CalendarStatus | null>(null);
  const [events, setEvents] = useState<CalendarEvent[]>([]);
  const [error, setError] = useState<string | null>(null);
  const live = useQrtzEvent('calendar:changed');
  const load = useCallback(() => {
    void qrtz
      .invoke('calendar:status')
      .then(setStatus, (e: AppError) => setError(e.message));
    void qrtz.invoke('calendar:upcoming').then(
      (r) => setEvents(r.events),
      () => setEvents([]),
    );
  }, [qrtz]);
  useEffect(load, [load]);
  useEffect(() => {
    if (!live) return;
    setStatus(live);
    void qrtz.invoke('calendar:upcoming').then(
      (r) => setEvents(r.events),
      () => setEvents([]),
    );
  }, [live, qrtz]);
  // The only call that asks macOS (ADR 0217): never on mount, only from a click.
  const connect = useCallback(() => {
    setError(null);
    return qrtz.invoke('calendar:connect').then(
      (s) => {
        setStatus(s);
        load();
      },
      (e: AppError) => setError(e.message),
    );
  }, [qrtz, load]);
  return { status, events, error, connect, reload: load };
}

const time = (ms: number) =>
  new Date(ms)
    .toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' })
    .toLowerCase();
const day = (ms: number) => {
  const d = new Date(ms);
  const today = new Date();
  const tomorrow = new Date(today.getTime() + 86_400_000);
  if (d.toDateString() === today.toDateString()) return 'today';
  if (d.toDateString() === tomorrow.toDateString()) return 'tomorrow';
  return d
    .toLocaleDateString([], {
      weekday: 'short',
      month: 'short',
      day: 'numeric',
    })
    .toLowerCase();
};

/** ADR 0217: the nano page's 'upcoming' block: the next meetings, or one 'connect calendar' prompt. */
export function Upcoming({
  onRecord,
  max = 5,
}: {
  onRecord: (title: string) => void;
  max?: number;
}) {
  const { status, events, error, connect } = useCalendar();
  if (!status || status.access === 'unsupported') return null;
  const on = status.enabled && status.access === 'granted';
  if (!on) {
    return (
      <Stack gap={2} data-testid="upcoming-connect">
        <Text variant="caption" muted>
          see your meetings here and name notes after them.
        </Text>
        <Button
          size="sm"
          variant="ghost"
          icon="calendar"
          onClick={() => void connect()}
          data-testid="calendar-connect"
        >
          connect calendar
        </Button>
        {error || (status.access !== 'not-determined' && accessNote(status)) ? (
          <Text variant="caption" muted data-testid="upcoming-access">
            {error ?? accessNote(status)}
          </Text>
        ) : null}
      </Stack>
    );
  }
  const shown = events.slice(0, max);
  return (
    <Stack gap={2} as="section" aria-label="upcoming" data-testid="upcoming">
      <Text variant="caption" muted>
        upcoming
      </Text>
      {shown.length === 0 ? (
        <Text variant="caption" muted data-testid="upcoming-empty">
          no meetings in the next two weeks
        </Text>
      ) : (
        <Stack as="ul" gap={1}>
          {shown.map((e) => (
            <Stack
              as="li"
              key={e.id}
              dir="row"
              gap={2}
              align="center"
              data-testid="upcoming-event"
            >
              <Stack gap={0} grow>
                <Text truncate data-testid="upcoming-title">
                  {e.title || 'untitled event'}
                </Text>
                <Text variant="caption" muted>
                  {day(e.startAt)} {time(e.startAt)}
                  {e.attendeeCount > 1 ? ` · ${e.attendeeCount} people` : ''}
                </Text>
              </Stack>
              {e.meetingUrl ? (
                <Icon name="video" size="sm" label="video call" />
              ) : null}
              <Button
                size="sm"
                variant="ghost"
                icon="circle"
                onClick={() => onRecord(e.title)}
                data-testid="upcoming-record"
                aria-label={`record ${e.title}`}
              >
                record
              </Button>
            </Stack>
          ))}
        </Stack>
      )}
    </Stack>
  );
}

/**
 * ADR 0217: nano settings → calendar: the switch (turning it on is what asks macOS), and which calendars count
 * (all by default; `note.calendar.calendars` null = all).
 */
export function CalendarSettings({
  patch,
}: {
  patch: (p: Record<string, unknown>) => void;
}) {
  const { status, error, connect } = useCalendar();
  if (!status) return null;
  const on = status.enabled && status.access === 'granted';
  const chosen = status.chosen;
  const isChosen = (id: string) => chosen == null || chosen.includes(id);
  const pick = (id: string, v: boolean) => {
    const all = status.calendars.map((c) => c.id);
    const cur = chosen ?? all;
    const next = v ? [...new Set([...cur, id])] : cur.filter((x) => x !== id);
    const everything = all.every((x) => next.includes(x));
    patch({ calendar: { calendars: everything ? null : next } });
  };
  const note = error ?? accessNote(status);
  return (
    <SettingsSection title="calendar" data-testid="note-settings-calendar">
      <SettingRow label="use apple calendar">
        <Switch
          label="use apple calendar"
          checked={on}
          disabled={status.access === 'unsupported'}
          onChange={(v) => {
            if (v) void connect();
            else patch({ calendar: { enabled: false } });
          }}
          data-testid="calendar-enabled"
        />
      </SettingRow>
      {on
        ? status.calendars.map((c) => (
            <SettingRow key={c.id} label={c.title.toLowerCase() || 'calendar'}>
              <Switch
                label={c.title.toLowerCase() || 'calendar'}
                checked={isChosen(c.id)}
                onChange={(v) => pick(c.id, v)}
                data-testid={`calendar-pick-${c.id}`}
              />
            </SettingRow>
          ))
        : null}
      {note && (status.enabled || status.access !== 'not-determined') ? (
        <Text variant="caption" muted data-testid="calendar-access">
          {note}
        </Text>
      ) : null}
      <Text variant="caption" muted>
        nano reads your calendar on this mac to name notes and offer to record
        when a meeting starts. it never changes your calendar and nothing leaves
        your mac.
      </Text>
    </SettingsSection>
  );
}
