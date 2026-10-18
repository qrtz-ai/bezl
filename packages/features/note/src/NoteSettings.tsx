import {
  Callout,
  Field,
  Input,
  Select,
  SettingRow,
  SettingsSection,
  Switch,
  Text,
} from '@qrtz/ui';
import { SUMMARY_TEMPLATES } from '@qrtz/core';
import { useModuleSettings } from '@qrtz/core/react';

/**
 * ADR 0146 M1.4: nano's own settings, inside nano's view: recording and summaries (`note`, in nano's settings
 * file), speaker labels (`speech.speakerMode`), and the two recording keys kept in the global `privacy` section.
 */
export default function NoteSettings() {
  const { settings, patch, error } = useModuleSettings();
  const note = (settings?.note ?? {}) as {
    retainAudioDays?: number;
    autoRecord?: boolean;
    autoDetectMeetings?: boolean;
    detectPrompt?: boolean;
    notifyNoAudio?: boolean;
    floatingHud?: boolean;
    autoSummarize?: boolean;
    defaultTemplate?: string;
  };
  const privacy = (settings?.privacy ?? {}) as {
    excludedApps?: string[];
    pauseHotkey?: string;
  };
  const speech = (settings?.speech ?? {}) as { speakerMode?: string };
  const setNote = (p: Record<string, unknown>) => void patch('note', p);
  const toggle = (
    label: string,
    testId: string,
    value: boolean | undefined,
    fallback: boolean,
    set: (v: boolean) => void,
  ) => (
    <SettingRow label={label}>
      <Switch
        label={label}
        checked={value ?? fallback}
        onChange={set}
        data-testid={testId}
      />
    </SettingRow>
  );
  if (!settings && !error) return null;
  const days = note.retainAudioDays ?? 0;
  return (
    <>
      {error ? (
        <Callout
          tone="danger"
          title={error}
          data-testid="note-settings-error"
        />
      ) : null}
      <SettingsSection title="recording" data-testid="note-settings-recording">
        {toggle(
          'detect meetings',
          'auto-detect',
          note.autoDetectMeetings,
          true,
          (v) => setNote({ autoDetectMeetings: v }),
        )}
        {toggle(
          'record automatically',
          'auto-record',
          note.autoRecord,
          false,
          (v) => setNote({ autoRecord: v }),
        )}
        {toggle(
          'ask to record',
          'detect-prompt',
          note.detectPrompt,
          true,
          (v) => setNote({ detectPrompt: v }),
        )}
        {toggle(
          'warn when no audio',
          'notify-no-audio',
          note.notifyNoAudio,
          true,
          (v) => setNote({ notifyNoAudio: v }),
        )}
        {toggle('floating hud', 'floating-hud', note.floatingHud, true, (v) =>
          setNote({ floatingHud: v }),
        )}
        {toggle(
          'delete old recordings',
          'retain-audio-on',
          days > 0,
          false,
          (v) => setNote({ retainAudioDays: v ? 30 : 0 }),
        )}
        {days > 0 ? (
          <SettingRow label="after">
            <Select
              value={String(days)}
              onChange={(e) =>
                setNote({ retainAudioDays: Number(e.target.value) })
              }
              data-testid="retain-audio"
            >
              <option value="7">7 days</option>
              <option value="30">30 days</option>
              <option value="90">90 days</option>
            </Select>
          </SettingRow>
        ) : null}
        <Field label="never record in">
          <Input
            key={(privacy.excludedApps ?? []).join(',')}
            defaultValue={(privacy.excludedApps ?? []).join(', ')}
            placeholder="safari, zoom"
            data-testid="excluded-apps"
            onBlur={(e) =>
              void patch('privacy', {
                excludedApps: e.target.value
                  .split(',')
                  .map((x) => x.trim())
                  .filter(Boolean),
              })
            }
          />
        </Field>
        <Field label="pause shortcut">
          <Input
            key={privacy.pauseHotkey ?? ''}
            defaultValue={privacy.pauseHotkey ?? 'CommandOrControl+Shift+P'}
            data-testid="pause-hotkey"
            onBlur={(e) =>
              void patch('privacy', { pauseHotkey: e.target.value.trim() })
            }
          />
        </Field>
      </SettingsSection>
      <SettingsSection title="speakers" data-testid="note-settings-speakers">
        <SettingRow label="speaker labels">
          <Select
            value={speech.speakerMode ?? 'fast'}
            onChange={(e) =>
              void patch('speech', { speakerMode: e.target.value })
            }
            data-testid="speaker-mode"
          >
            <option value="fast">fast (about 1 s behind)</option>
            <option value="accurate">accurate (about 3 s behind)</option>
          </Select>
        </SettingRow>
      </SettingsSection>
      <SettingsSection title="summaries" data-testid="note-settings-summaries">
        {toggle(
          'summarize after meetings',
          'auto-summarize',
          note.autoSummarize,
          true,
          (v) => setNote({ autoSummarize: v }),
        )}
        <SettingRow label="template">
          <Select
            value={note.defaultTemplate ?? 'general'}
            onChange={(e) => setNote({ defaultTemplate: e.target.value })}
            data-testid="default-template"
          >
            {SUMMARY_TEMPLATES.map((tp) => (
              <option key={tp.id} value={tp.id}>
                {tp.label}
              </option>
            ))}
          </Select>
        </SettingRow>
        <Text variant="caption" muted data-testid="summary-availability">
          summaries are written on this mac by apple intelligence. nothing is
          sent anywhere.
        </Text>
      </SettingsSection>
    </>
  );
}
