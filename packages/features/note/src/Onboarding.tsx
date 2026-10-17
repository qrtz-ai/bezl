import { Button, Heading, Page, PermissionList, Stack } from '@qrtz/ui';
import type { OsPermission, PermissionsStatus } from '@qrtz/core';

const COPY: Record<OsPermission, string> = {
  accessibility: 'Accessibility',
  'screen-recording': 'Screen & System Audio',
  microphone: 'Microphone',
};

export const needsOnboarding = (p: PermissionsStatus) =>
  Object.values(p).some((s) => s === 'denied' || s === 'not-determined');

export function Onboarding({
  status,
  onRequest,
  onDone,
}: {
  status: PermissionsStatus;
  onRequest: (p: OsPermission) => void;
  onDone: () => void;
}) {
  return (
    <Page width={480} data-testid="note-onboarding">
      <Heading cursor>set up note</Heading>
      <PermissionList
        items={(Object.keys(COPY) as OsPermission[]).map((p) => {
          const st = status[p] ?? 'unknown';
          const ok = st === 'granted' || st === 'unsupported';
          return {
            id: p,
            label: COPY[p],
            state: st === 'granted' ? 'Granted' : 'Not needed',
            ...(ok ? {} : { onGrant: () => onRequest(p) }),
          };
        })}
      />
      <Stack dir="row">
        <Button
          variant="ghost"
          onClick={onDone}
          data-testid="onboarding-continue"
        >
          Continue
        </Button>
      </Stack>
    </Page>
  );
}
