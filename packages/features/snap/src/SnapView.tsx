import { useCallback, useEffect, useState } from 'react';
import {
  Badge,
  Callout,
  Heading,
  Kbd,
  ModuleSettingsButton,
  ModuleSettingsPage,
  Page,
  SettingRow,
  SettingsSection,
  Stack,
  Text,
} from '@qrtz/ui';
import {
  SNAP_GROUPS,
  SNAP_LABELS,
  shortcutSymbols,
  type SnapStatus,
} from '@qrtz/core';
import { useQrtz, useQrtzEvent, useSubAppRoute } from '@qrtz/core/react';
import { SETTINGS_PATH } from '@qrtz/core';
import SnapSettings from './SnapSettings';

/**
 * qrtz snap (ADR 0159): every Spectacle action with the shortcut qrtz-snap holds for it. The shortcut editor and the
 * options are snap's own settings page (the gear), not the global settings.
 */
export default function SnapView() {
  const qrtz = useQrtz();
  const changed = useQrtzEvent('snap:changed');
  const [settingsOpen, setSettingsOpen] = useState(false);
  // ADR 0161: the module manager's "settings" opens this view at `settings`.
  const route = useSubAppRoute();
  useEffect(() => {
    if (route.path === SETTINGS_PATH) setSettingsOpen(true);
  }, [route.path]);
  const closeSettings = () => {
    setSettingsOpen(false);
    if (route.path === SETTINGS_PATH) route.navigate('', { replace: true });
  };
  const [status, setStatus] = useState<SnapStatus | null>(null);
  const load = useCallback(
    () =>
      void qrtz.invoke('snap:status').then(setStatus, () => setStatus(null)),
    [qrtz],
  );
  useEffect(load, [load, changed, settingsOpen]);

  if (settingsOpen)
    return (
      <ModuleSettingsPage
        module="snap"
        onClose={closeSettings}
        data-testid="snap-settings"
      >
        <SnapSettings status={status} onSaved={load} />
      </ModuleSettingsPage>
    );
  return (
    <Page data-testid="snap">
      <Stack dir="row" align="center" justify="between">
        <Heading>snap</Heading>
        <Stack dir="row" gap={2} align="center">
          <Text variant="caption" muted>
            move and resize windows from the keyboard
          </Text>
          <ModuleSettingsButton
            module="snap"
            onClick={() => setSettingsOpen(true)}
            data-testid="snap-settings-open"
          />
        </Stack>
      </Stack>
      {status && !status.running ? (
        <Callout
          tone="info"
          title="snap isn't running"
          data-testid="snap-not-running"
        >
          it starts when snap is on and qrtz is open.
        </Callout>
      ) : null}
      {status?.running && !status.axTrusted ? (
        <Callout
          tone="warning"
          title="snap needs accessibility access"
          data-testid="snap-no-ax"
        >
          give qrtz accessibility access in system settings → privacy &
          security. snap uses qrtz's one grant.
        </Callout>
      ) : null}
      {SNAP_GROUPS.map((group, i) => (
        <SettingsSection
          key={group.join()}
          title={
            [
              'position',
              'halves',
              'corners',
              'displays',
              'thirds',
              'size',
              'history',
            ][i] ?? ''
          }
          data-testid={`snap-group-${i}`}
        >
          {group.map((a) => {
            const keys = status?.registered[a];
            const why = status?.refused[a];
            return (
              <SettingRow key={a} label={SNAP_LABELS[a]}>
                {keys ? (
                  <Kbd>{shortcutSymbols(keys)}</Kbd>
                ) : why ? (
                  <Badge tone="warning">{why}</Badge>
                ) : (
                  <Text variant="caption" muted>
                    no shortcut
                  </Text>
                )}
              </SettingRow>
            );
          })}
        </SettingsSection>
      ))}
    </Page>
  );
}
