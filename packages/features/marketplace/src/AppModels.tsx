import { useCallback, useEffect, useState } from 'react';
import {
  Button,
  Dialog,
  ModelsPanel,
  SettingsSection,
  Stack,
  Text,
  userError,
  type UserError,
} from '@qrtz/ui';
import type { SpeechSetupStatus } from '@qrtz/core';
import { useQrtz } from '@qrtz/core/react';
import { keptFor, type ModelApp } from './catalog';

/** The host refuses a per-app delete of the one shared speech bundle (setup.rs `delete_for`, a remote source). */
const isSharedBundle = (m: string) => /speech module is shared/i.test(m);

/**
 * An app's models in its detail modal on the qrtz page (they moved here from settings → speech). Every call is
 * scoped to this app (`extensions: [id]`, ADR 0175): status, download, pause, resume and delete reach only
 * its own sets, and the host's scoped delete keeps any set another switched-on app still needs (shown as "kept,
 * used by nano"). There is no unscoped "delete all speech" anywhere in the UI.
 */
export function AppModels({
  app,
  on,
}: {
  app: ModelApp;
  /** The apps switched on right now (whose shared sets a delete keeps). */
  on: readonly string[];
}) {
  const qrtz = useQrtz();
  const [status, setStatus] = useState<SpeechSetupStatus | null>(null);
  const [error, setError] = useState<UserError | null>(null);
  const [confirming, setConfirming] = useState(false);
  const scope = { extensions: [app] };
  const read = useCallback(
    () =>
      qrtz
        .invoke('speech:status', { extensions: [app] })
        .then(setStatus, (e: unknown) =>
          setError(userError(e, `read ${app}'s models`)),
        ),
    [qrtz, app],
  );
  useEffect(() => {
    void read();
    // The broadcast status is the whole store's: read this app's own again.
    return qrtz.on('speech:status', () => void read());
  }, [qrtz, read]);
  const run = (
    p: Promise<SpeechSetupStatus>,
    what: string,
    friendly?: (m: string) => string | null,
  ) => {
    setError(null);
    p.then(setStatus, (e: Error) => {
      const plain = friendly?.(e.message);
      setError(
        plain
          ? { title: `couldn't ${what}`, next: plain, detail: null }
          : userError(e, what),
      );
    });
  };
  const sets = keptFor(app, on);
  const kept = sets.filter((s) => s.keptFor.length);
  return (
    <SettingsSection title="models" data-testid={`models-${app}`}>
      <ModelsPanel
        status={status}
        sets={sets}
        error={error}
        data-testid={`models-${app}-panel`}
        // Every set is someone else's too: a delete would remove nothing.
        deleteBlocked={sets.length > 0 && kept.length === sets.length}
        onDownload={() =>
          run(qrtz.invoke('speech:download', scope), 'download the models')
        }
        onPause={() =>
          run(qrtz.invoke('speech:pause', scope), 'pause the download')
        }
        onResume={() =>
          run(qrtz.invoke('speech:resume', scope), 'resume the download')
        }
        onDelete={() => setConfirming(true)}
      />
      <Dialog
        open={confirming}
        onClose={() => setConfirming(false)}
        title={`delete ${app}'s models`}
      >
        <Stack gap={4} pad={5} data-testid={`models-${app}-confirm`}>
          <Text muted>{`${app} can't run until you download them again.`}</Text>
          {kept.length ? (
            <Text
              variant="caption"
              muted
              data-testid={`models-${app}-confirm-kept`}
            >
              {kept
                .map(
                  (s) => `${s.name} is kept, used by ${s.keptFor.join(', ')}`,
                )
                .join('. ')}
              .
            </Text>
          ) : null}
          <Stack dir="row" gap={2} justify="end">
            <Button variant="ghost" onClick={() => setConfirming(false)}>
              cancel
            </Button>
            <Button
              variant="danger"
              icon="trash"
              onClick={() => {
                setConfirming(false);
                run(
                  qrtz.invoke('speech:delete', scope),
                  'delete the models',
                  (m) =>
                    isSharedBundle(m)
                      ? "these models are part of bezl's shared speech, so they can't be deleted for one app."
                      : null,
                );
              }}
              data-testid={`models-${app}-confirm-delete`}
            >
              delete models
            </Button>
          </Stack>
        </Stack>
      </Dialog>
    </SettingsSection>
  );
}
