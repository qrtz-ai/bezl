import { useState } from 'react';
import {
  Badge,
  Button,
  Callout,
  Input,
  Kbd,
  Select,
  SettingRow,
  SettingsSection,
  Stack,
  Switch,
  Text,
} from '@qrtz/ui';
import {
  SNAP_DEFAULT_SHORTCUTS,
  SNAP_GROUPS,
  SNAP_LABELS,
  bindingFromKey,
  shortcutSymbols,
  type SnapAction,
  type SnapStatus,
} from '@qrtz/core';

export type SnapSettingsSection = {
  shortcuts?: Record<string, string>;
  disabledApps?: string[];
  statusItem?: boolean;
  summon?: { enabled?: boolean; bundleId?: string; name?: string };
};

/** The binding an action has after the user's changes ("" = cleared). */
export function effectiveBinding(
  overrides: Record<string, string> | undefined,
  a: SnapAction,
): string {
  return overrides && a in overrides
    ? (overrides[a] ?? '')
    : SNAP_DEFAULT_SHORTCUTS[a];
}

/** A section title for each of SNAP_GROUPS, in order. */
const GROUP_TITLES = [
  'position',
  'halves',
  'corners',
  'displays',
  'thirds',
  'size',
  'history',
];

/**
 * Snap's main page (Jared): Spectacle's shortcut editor, grouped as before. Click a shortcut, press the new keys;
 * clear it, or put back the default. Stored in snap's settings file (config `snap`).
 */
export function ShortcutSections({
  shortcuts,
  status,
  onChange,
}: {
  shortcuts: Record<string, string> | undefined;
  status: SnapStatus | null;
  onChange: (shortcuts: Record<string, string>) => void;
}) {
  const [recording, setRecording] = useState<SnapAction | null>(null);
  const setShortcut = (a: SnapAction, binding: string) =>
    onChange({ ...shortcuts, [a]: binding });
  const held = status?.registered ? Object.keys(status.registered).length : 0;
  return (
    <>
      <SettingsSection
        title="shortcuts"
        description="click a shortcut, then press the new keys. it needs ⌘, ⌥ or ⌃."
        data-testid="snap-settings-shortcuts"
      >
        <SettingRow
          label="spectacle's shortcuts"
          description={
            held ? (
              <>
                snap holds <Kbd>{String(held)}</Kbd> shortcuts right now.
              </>
            ) : undefined
          }
        >
          <Button
            variant="secondary"
            onClick={() => onChange({ ...SNAP_DEFAULT_SHORTCUTS })}
            data-testid="snap-restore-all"
          >
            restore all
          </Button>
        </SettingRow>
      </SettingsSection>
      {SNAP_GROUPS.map((group, i) => (
        <SettingsSection
          key={group.join()}
          title={GROUP_TITLES[i] ?? ''}
          data-testid={`snap-group-${i}`}
        >
          {group.map((a) => {
            const binding = effectiveBinding(shortcuts, a);
            const why = status?.refused[a];
            return (
              <SettingRow key={a} label={SNAP_LABELS[a]}>
                <Stack dir="row" gap={1} align="center">
                  {why ? <Badge tone="warning">{why}</Badge> : null}
                  <Input
                    aria-label={`${SNAP_LABELS[a]} shortcut`}
                    readOnly
                    value={
                      recording === a
                        ? 'press keys…'
                        : binding
                          ? shortcutSymbols(binding)
                          : 'none'
                    }
                    onFocus={() => setRecording(a)}
                    onBlur={() => setRecording(null)}
                    onKeyDown={(e) => {
                      if (recording !== a) return;
                      e.preventDefault();
                      if (e.key === 'Escape') return setRecording(null);
                      const b = bindingFromKey(e);
                      if (!b) return;
                      setRecording(null);
                      setShortcut(a, b);
                      e.currentTarget.blur();
                    }}
                    data-testid={`snap-shortcut-${a}`}
                  />
                  <Button
                    variant="ghost"
                    size="sm"
                    icon="close"
                    aria-label={`clear ${SNAP_LABELS[a]}`}
                    onClick={() => setShortcut(a, '')}
                    data-testid={`snap-clear-${a}`}
                  />
                  <Button
                    variant="ghost"
                    size="sm"
                    icon="undo"
                    aria-label={`restore ${SNAP_LABELS[a]}`}
                    disabled={binding === SNAP_DEFAULT_SHORTCUTS[a]}
                    onClick={() => setShortcut(a, SNAP_DEFAULT_SHORTCUTS[a])}
                    data-testid={`snap-default-${a}`}
                  />
                </Stack>
              </SettingRow>
            );
          })}
        </SettingsSection>
      ))}
    </>
  );
}

/**
 * ADR 0159: snap's opts modal (the gear): only the menu bar item and the apps snap leaves alone. The shortcuts and
 * summon live on snap's main page.
 */
export default function SnapSettings({
  snap,
  set,
  error,
}: {
  snap: SnapSettingsSection;
  set: (p: SnapSettingsSection) => void;
  error: string | null;
}) {
  const [app, setApp] = useState('');
  const apps = snap.disabledApps ?? [];
  return (
    <>
      {error ? (
        <Callout
          tone="danger"
          title={error}
          data-testid="snap-settings-error"
        />
      ) : null}
      <SettingsSection title="menu bar" data-testid="snap-settings-menu">
        <SettingRow label="show snap in the menu bar">
          <Switch
            label="show snap in the menu bar"
            checked={snap.statusItem ?? true}
            onChange={(v) => set({ statusItem: v })}
            data-testid="snap-status-item"
          />
        </SettingRow>
      </SettingsSection>
      <SettingsSection
        title="apps snap leaves alone"
        description="photoshop and steam are always left alone. you can also turn snap off for the front app from its menu bar item."
        data-testid="snap-settings-apps"
      >
        {apps.map((id) => (
          <SettingRow key={id} label={id}>
            <Button
              variant="ghost"
              size="sm"
              icon="trash"
              aria-label={`stop leaving ${id} alone`}
              onClick={() =>
                set({ disabledApps: apps.filter((x) => x !== id) })
              }
              data-testid={`snap-app-remove-${id}`}
            />
          </SettingRow>
        ))}
        <Stack
          as="form"
          dir="row"
          gap={2}
          onSubmit={() => {
            const id = app.trim();
            if (!id || apps.includes(id)) return;
            set({ disabledApps: [...apps, id] });
            setApp('');
          }}
        >
          <Input
            aria-label="app bundle id"
            placeholder="com.example.app"
            value={app}
            maxLength={255}
            onChange={(e) => setApp(e.target.value)}
            data-testid="snap-app-input"
          />
          <Button type="submit" variant="secondary" data-testid="snap-app-add">
            add
          </Button>
        </Stack>
      </SettingsSection>
    </>
  );
}

/**
 * Summon agent: hold left ⌘ and right ⌘ together (nothing else) to bring the chosen app forward; do it again to go
 * back to the app you were in. qrtz-snap watches the chord with a listen-only tap, which needs Input Monitoring:
 * macOS asks the moment this turns on, never before.
 */
export function SummonSection({
  summon,
  status,
  onChange,
}: {
  summon: { enabled?: boolean; bundleId?: string; name?: string };
  status: SnapStatus | null;
  onChange: (p: {
    enabled?: boolean;
    bundleId?: string;
    name?: string;
  }) => void;
}) {
  const on = summon.enabled ?? false;
  const apps = status?.apps ?? [];
  const chosen = summon.bundleId ?? '';
  const listed = apps.some((a) => a.bundleId === chosen);
  return (
    <SettingsSection
      title="summon agent"
      description="hold left ⌘ and right ⌘ together to bring an app forward. do it again to go back."
      data-testid="snap-settings-summon"
    >
      <SettingRow label="summon agent">
        <Switch
          label="summon agent"
          checked={on}
          onChange={(v) => onChange({ enabled: v })}
          data-testid="snap-summon-enabled"
        />
      </SettingRow>
      {on ? (
        <>
          <SettingRow label="app">
            <Select
              aria-label="app to summon"
              value={chosen}
              onChange={(e) => {
                const a = apps.find((x) => x.bundleId === e.target.value);
                onChange({ bundleId: a?.bundleId ?? '', name: a?.name ?? '' });
              }}
              data-testid="snap-summon-app"
            >
              <option value="">choose an app</option>
              {chosen && !listed ? (
                <option value={chosen}>{summon.name || chosen}</option>
              ) : null}
              {apps.map((a) => (
                <option key={a.bundleId} value={a.bundleId}>
                  {a.name}
                </option>
              ))}
            </Select>
          </SettingRow>
          {status?.running ? (
            status.inputMonitoring ? (
              <Text variant="caption" muted data-testid="snap-summon-ready">
                input monitoring is on.{' '}
                {status.summonActive
                  ? 'the chord is ready.'
                  : 'starting the chord…'}
              </Text>
            ) : (
              <Callout
                tone="warning"
                title="summon needs input monitoring"
                data-testid="snap-summon-no-im"
              >
                turn on qrtz in system settings → privacy & security → input
                monitoring. if qrtz is already on there, quit qrtz and open it
                again.
              </Callout>
            )
          ) : null}
          {status?.summonClash ? (
            <Callout
              tone="warning"
              title="summon is paused: right ⌘ is echo's dictation key"
              data-testid="snap-summon-clash"
            >
              summon stays off while echo uses right ⌘ to dictate, so a chord
              can't also start dictation. pick another key in echo's settings to
              use summon.
            </Callout>
          ) : null}
        </>
      ) : null}
    </SettingsSection>
  );
}
