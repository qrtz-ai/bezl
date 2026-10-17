import { useEffect, useMemo, useState } from 'react';
import {
  Button,
  Callout,
  Field,
  Input,
  OnboardingStep,
  PermissionList,
  SpeechSetupPanel,
  Stack,
} from '@qrtz/ui';
import type { OsPermission, PermissionsStatus } from '@qrtz/core';
import {
  QrtzClientProvider,
  useQrtz,
  useQrtzEvent,
  useSpeechSetup,
} from '@qrtz/core/react';
import { shellClient } from './shellClient';

const STEPS = 5;
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

/** First run (shell-owned, sub apps unmounted): welcome → permissions → speech → openrouter key → done. */
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
  const [key, setKey] = useState('');
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
  const saveKey = () =>
    void qrtz
      .invoke('secrets:set', { name: 'llm.openrouter', value: key.trim() })
      .then(
        () => {
          setKey('');
          next();
        },
        (e: Error) => setError(e.message),
      );

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
  if (step === 4)
    return (
      <OnboardingStep
        step={4}
        steps={STEPS}
        title="summaries"
        data-testid="onboarding-key"
        actions={
          <>
            <Button
              variant="ghost"
              onClick={next}
              data-testid="onboarding-skip"
            >
              skip
            </Button>
            <Button
              disabled={!key.trim()}
              onClick={saveKey}
              data-testid="onboarding-save-key"
            >
              save
            </Button>
          </>
        }
      >
        {error ? alert : null}
        <Stack gap={2}>
          <Field label="openrouter key (optional)">
            <Input
              type="password"
              autoComplete="off"
              placeholder="sk-or-…"
              value={key}
              onChange={(e) => setKey(e.target.value)}
              data-testid="onboarding-key-input"
            />
          </Field>
        </Stack>
      </OnboardingStep>
    );
  return (
    <OnboardingStep
      step={5}
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
