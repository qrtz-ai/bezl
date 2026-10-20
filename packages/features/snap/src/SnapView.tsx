import { useCallback, useEffect, useState } from 'react';
import {
  Badge,
  Callout,
  Kbd,
  ModuleSettingsButton,
  ModuleSettingsModal,
  Page,
  PermissionPrompt,
  SettingRow,
  SettingsSection,
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
 * options are snap's own opts modal (the gear), not the global settings.
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
  // The accessibility prompt is showing: read the grant every second (qrtz-app reads it live), so the prompt goes
  // away and the shortcuts work the moment the user switches qrtz on, no relaunch.
  const needsAx = !!status?.running && !status.axTrusted;
  const [waiting, setWaiting] = useState(false);
  useEffect(() => {
    if (!needsAx) {
      setWaiting(false);
      return;
    }
    const id = setInterval(load, 1000);
    return () => clearInterval(id);
  }, [needsAx, load]);
  const askAx = () => {
    setWaiting(true);
    void qrtz.invoke('snap:request-access').then(load, () => {});
  };

  const optsModal = (
    <ModuleSettingsModal
      open={settingsOpen}
      module="snap"
      onClose={closeSettings}
      data-testid="snap-settings"
    >
      <SnapSettings status={status} onSaved={load} />
    </ModuleSettingsModal>
  );
  return (
    <Page
      data-testid="snap"
      title="snap"
      actions={
        <>
          <Text variant="caption" muted truncate>
            move and resize windows from the keyboard
          </Text>
          <ModuleSettingsButton
            module="snap"
            onClick={() => setSettingsOpen(true)}
            data-testid="snap-settings-open"
          />
        </>
      }
    >
      {optsModal}
      {status && !status.running ? (
        <Callout
          tone="info"
          title="snap isn't running"
          data-testid="snap-not-running"
        >
          it starts when snap is on and qrtz is open.
        </Callout>
      ) : null}
      {needsAx ? (
        <PermissionPrompt
          title="snap needs accessibility access"
          pane="accessibility"
          onOpen={askAx}
          waiting={waiting}
          data-testid="snap-no-ax"
        />
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
