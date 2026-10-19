import {
  Callout,
  Field,
  Input,
  Select,
  SettingRow,
  SettingsSection,
  Switch,
} from '@qrtz/ui';
import { useModuleSettings } from '@qrtz/core/react';
import { PillDevOpts } from './PillDevOpts';

/** ADR 0146 M1.4: echo's own settings (the `talk` section, kept in echo's settings file), inside echo's view. */
export default function TalkSettings() {
  const { settings, patch, error } = useModuleSettings();
  const talk = (settings?.talk ?? {}) as {
    enabled?: boolean;
    pushKey?: string;
    blockGlobeKey?: boolean;
    toggleHotkey?: string;
    warmMic?: boolean;
    autoCleanup?: boolean;
    autoInsert?: boolean;
    restoreClipboard?: boolean;
    showPill?: boolean;
    echoGate?: boolean;
    saveHistory?: boolean;
    pillMotion?: unknown;
  };
  const set = (p: Record<string, unknown>) => void patch('talk', p);
  const toggle = (
    label: string,
    testId: string,
    value: boolean | undefined,
    fallback: boolean,
    onChange: (v: boolean) => void,
  ) => (
    <SettingRow label={label}>
      <Switch
        label={label}
        checked={value ?? fallback}
        onChange={onChange}
        data-testid={testId}
      />
    </SettingRow>
  );
  if (!settings && !error) return null;
  return (
    <>
      {error ? (
        <Callout
          tone="danger"
          title={error}
          data-testid="talk-settings-error"
        />
      ) : null}
      <SettingsSection title="dictation" data-testid="talk-settings-dictation">
        {toggle('dictation', 'talk-enabled', talk.enabled, true, (v) =>
          set({ enabled: v }),
        )}
        <SettingRow label="hold to dictate">
          <Select
            value={talk.pushKey ?? 'fn'}
            onChange={(e) => set({ pushKey: e.target.value })}
            data-testid="talk-push-key"
          >
            <option value="fn">fn</option>
            <option value="right-option">right ⌥</option>
            <option value="right-command">right ⌘</option>
            <option value="right-control">right ⌃</option>
            <option value="">off</option>
          </Select>
        </SettingRow>
        {(talk.pushKey ?? 'fn') === 'fn'
          ? toggle(
              'fn only for echo (no input switch or emoji)',
              'talk-block-globe',
              talk.blockGlobeKey,
              true,
              (v) => set({ blockGlobeKey: v }),
            )
          : null}
        <Field label="toggle shortcut">
          <Input
            key={talk.toggleHotkey ?? ''}
            defaultValue={talk.toggleHotkey ?? 'CommandOrControl+Shift+Space'}
            data-testid="talk-toggle-hotkey"
            onBlur={(e) => set({ toggleHotkey: e.target.value.trim() })}
          />
        </Field>
        {toggle('warm mic', 'talk-warm-mic', talk.warmMic, false, (v) =>
          set({ warmMic: v }),
        )}
        {toggle(
          'ignore the speakers',
          'talk-echo-gate',
          talk.echoGate,
          true,
          (v) => set({ echoGate: v }),
        )}
      </SettingsSection>
      <SettingsSection title="text" data-testid="talk-settings-text">
        {toggle(
          'clean up text',
          'talk-auto-cleanup',
          talk.autoCleanup,
          true,
          (v) => set({ autoCleanup: v }),
        )}
        {toggle(
          'insert into app',
          'talk-auto-insert',
          talk.autoInsert,
          true,
          (v) => set({ autoInsert: v }),
        )}
        {toggle(
          'restore clipboard',
          'talk-restore-clipboard',
          talk.restoreClipboard,
          true,
          (v) => set({ restoreClipboard: v }),
        )}
        {toggle('floating pill', 'talk-show-pill', talk.showPill, true, (v) =>
          set({ showPill: v }),
        )}
        {toggle(
          'keep history',
          'talk-save-history',
          talk.saveHistory,
          true,
          (v) => set({ saveHistory: v }),
        )}
      </SettingsSection>
      <PillDevOpts
        value={talk.pillMotion}
        onCommit={(m) => set({ pillMotion: m })}
      />
    </>
  );
}
