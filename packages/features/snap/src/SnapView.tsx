import { useCallback, useEffect, useState } from 'react';
import {
  Callout,
  ModuleSettingsButton,
  ModuleSettingsModal,
  Page,
  PermissionPrompt,
  Text,
} from '@qrtz/ui';
import { type SnapStatus } from '@qrtz/core';
import {
  useModuleSettings,
  useQrtz,
  useQrtzEvent,
  useSubAppRoute,
} from '@qrtz/core/react';
import { SETTINGS_PATH } from '@qrtz/core';
import SnapSettings, {
  ShortcutSections,
  SummonSection,
  type SnapSettingsSection,
} from './SnapSettings';

/**
 * qrtz snap (ADR 0159): the summon agent and every Spectacle action with its editable shortcut are the main page.
 * Snap's opts modal (the gear) keeps only the menu bar item and the apps snap leaves alone. One settings read serves
 * both, so a change on either side shows on the other.
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
  const { settings, patch, error: settingsError } = useModuleSettings();
  const snap = (settings?.snap ?? {}) as SnapSettingsSection;
  const set = (p: SnapSettingsSection) => void patch('snap', p).then(load);
  // The accessibility prompt is showing: read the grant every second (qrtz-app reads it live), so the prompt goes
  // away and the shortcuts work the moment the user switches qrtz on, no relaunch.
  const needsAx = !!status?.running && !status.axTrusted;
  const needsIm =
    !!status?.running && !!snap.summon?.enabled && !status.inputMonitoring;
  const [waiting, setWaiting] = useState(false);
  useEffect(() => {
    if (!needsAx) {
      setWaiting(false);
      return;
    }
    const id = setInterval(load, 1000);
    return () => clearInterval(id);
  }, [needsAx, load]);
  // Summon's input monitoring prompt: the same live read every second once asked.
  const [waitingIm, setWaitingIm] = useState(false);
  useEffect(() => {
    if (!needsIm) {
      setWaitingIm(false);
      return;
    }
    if (!waitingIm) return;
    const id = setInterval(load, 1000);
    return () => clearInterval(id);
  }, [needsIm, waitingIm, load]);
  const askIm = () => {
    setWaitingIm(true);
    void qrtz
      .invoke('snap:request-access', { permission: 'input-monitoring' })
      .then(load, () => {});
  };
  const askAx = () => {
    setWaiting(true);
    void qrtz.invoke('snap:request-access', undefined).then(load, () => {});
  };

  const optsModal = (
    <ModuleSettingsModal
      open={settingsOpen}
      module="snap"
      onClose={closeSettings}
      data-testid="snap-settings"
    >
      {settings || settingsError ? (
        <SnapSettings snap={snap} set={set} error={settingsError} />
      ) : null}
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
      {settingsError ? (
        <Callout
          tone="danger"
          title={settingsError}
          data-testid="snap-main-error"
        />
      ) : null}
      {settings ? (
        <>
          <SummonSection
            summon={snap.summon ?? {}}
            status={status}
            onChange={(p) => set({ summon: { ...snap.summon, ...p } })}
            onGrantInputMonitoring={askIm}
            waiting={waitingIm}
          />
          <ShortcutSections
            shortcuts={snap.shortcuts}
            status={status}
            onChange={(shortcuts) => set({ shortcuts })}
          />
        </>
      ) : null}
    </Page>
  );
}
