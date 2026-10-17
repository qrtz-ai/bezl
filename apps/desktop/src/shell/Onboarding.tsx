import { useEffect, useMemo, useState } from 'react';
import {
  Button,
  Callout,
  OnboardingStep,
  PermissionList,
  SpeechSetupPanel,
} from '@qrtz/ui';
import type { OsPermission, PermissionsStatus } from '@qrtz/core';
import {
  QrtzClientProvider,
  useQrtz,
  useQrtzEvent,
  useSpeechSetup,
} from '@qrtz/core/react';
import { shellClient } from './shellClient';

const STEPS = 4;
const PERMISSIONS: { id: OsPermission; label: string }[] = [
  { id: 'microphone', label: 'microphone' },
  { id: 'screen-recording', label: 'system audio' },
  { id: 'accessibility', label: 'accessibility (talk)' },
];
const PERMISSION_STATE: Record<string, string> = {
  granted: 'allowed',
  unsupported: 'not needed',
  unknown: 'checking…',
};

/** First run (shell-owned, sub apps unmounted): welcome → permissions → speech → done. qrtz is
 * entirely offline (ADR 0076), so there is no key step. */
export function Onboarding({ onDone }: { onDone: () => void }) {
  const client = useMemo(shellClient, []);
  return (
    <QrtzClientProvider client={client}>
      <Steps onDone={onDone} />
    </QrtzClientProvider>
  );
}

function Steps({ onDone }: { onDone: () => void }) {
  const qrtz = useQrtz();
  const [step, setStep] = useState(1);
  const [perms, setPerms] = useState<PermissionsStatus | null>(null);
  const livePerms = useQrtzEvent('permissions:changed');
  const setup = useSpeechSetup();
  const [error, setError] = useState<string | null>(null);
  const next = () => {
    setError(null);
    setStep((s) => s + 1);
  };

  const refreshPerms = () =>
    void qrtz.invoke('permissions:get').then(setPerms, (e: Error) => {
      setError(e.message);
    });
  useEffect(() => {
    if (step !== 2) return;
    refreshPerms();
    // Granted in System Settings while qrtz was in the background: re-read on return.
    window.addEventListener('focus', refreshPerms);
    return () => window.removeEventListener('focus', refreshPerms);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [step, qrtz]);
  useEffect(() => {
    if (livePerms) setPerms(livePerms);
  }, [livePerms]);

  const finish = () =>
    void qrtz
      .invoke('config:set', { patch: { onboarding: { done: true } } })
      .then(onDone, (e: Error) => setError(e.message));

  const problem = error ?? setup.error;
  const alert = problem ? (
    <Callout tone="danger" title={problem} data-testid="onboarding-error" />
  ) : null;

  if (step === 1)
    return (
      <OnboardingStep
        step={1}
        steps={STEPS}
        title="welcome to qrtz"
        data-testid="onboarding-welcome"
        actions={
          <Button onClick={next} data-testid="onboarding-start">
            start
          </Button>
        }
      />
    );
  if (step === 2)
    return (
      <OnboardingStep
        step={2}
        steps={STEPS}
        title="permissions"
        data-testid="onboarding-permissions"
        actions={
          <Button onClick={next} data-testid="onboarding-continue">
            continue
          </Button>
        }
      >
        {alert}
        <PermissionList
          items={PERMISSIONS.map(({ id, label }) => {
            const st = perms?.[id] ?? 'unknown';
            const grant = st === 'denied' || st === 'not-determined';
            return {
              id,
              label,
              state: PERMISSION_STATE[st] ?? st,
              ...(grant
                ? {
                    onGrant: () =>
                      void qrtz
                        .invoke('permissions:request', { permission: id })
                        .then(setPerms, (e: Error) => setError(e.message)),
                  }
                : {}),
            };
          })}
        />
      </OnboardingStep>
    );
  if (step === 3) {
    const s = setup.speech;
    const started = !!s?.installed || (s?.download.state ?? 'idle') !== 'idle';
    return (
      <OnboardingStep
        step={3}
        steps={STEPS}
        title="speech"
        data-testid="onboarding-speech"
        actions={
          <Button
            variant={started ? 'primary' : 'ghost'}
            onClick={next}
            data-testid="onboarding-continue"
          >
            {started ? 'continue' : 'later'}
          </Button>
        }
      >
        {setup.error ? alert : null}
        <SpeechSetupPanel
          status={s}
          onDownload={setup.download}
          onPause={setup.pause}
          onResume={setup.resume}
        />
      </OnboardingStep>
    );
  }
  return (
    <OnboardingStep
      step={4}
      steps={STEPS}
      title="you're set"
      data-testid="onboarding-done"
      actions={
        <Button onClick={finish} data-testid="onboarding-finish">
          open qrtz
        </Button>
      }
    >
      {error ? alert : null}
    </OnboardingStep>
  );
}
