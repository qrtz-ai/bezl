import type { Meta, StoryObj } from '@storybook/react-vite';
import type { AppUpdateStatus, QrtzClient } from '@qrtz/core';
import { QrtzClientProvider } from '@qrtz/core/react';
import { ThemeProvider } from '@qrtz/ui';
import AppUpdateRow from '@qrtz/feature-settings/app-update-row';

/** settings → qrtz's update row in its coded failure states (QRT-313), on a fixed status instead of the mock backend. */
const meta: Meta = { title: 'states/app update' };
export default meta;

const status = (p: Partial<AppUpdateStatus>): AppUpdateStatus => ({
  current: '0.2.4',
  state: 'failed',
  latest: '0.2.5',
  notes: null,
  sizeBytes: null,
  doneBytes: null,
  error: 'update_redirect_blocked',
  errorCode: 'update_redirect_blocked',
  errorDetails: { host: 'downloads.example.net', scheme: 'https' },
  checkedAtMs: null,
  ...p,
});

const client = (s: AppUpdateStatus) =>
  ({
    subApp: 'settings',
    invoke: async (channel: string) =>
      channel === 'app-update:status' ? s : s,
    on: () => () => {},
  }) as unknown as QrtzClient;

const row = (s: AppUpdateStatus): StoryObj => ({
  render: () => (
    <ThemeProvider mode="dark">
      <QrtzClientProvider client={client(s)}>
        <AppUpdateRow />
      </QrtzClientProvider>
    </ThemeProvider>
  ),
});

export const RedirectBlocked = row(status({}));
export const RedirectBlockedHttp = row(
  status({ errorDetails: { host: 'downloads.example.net', scheme: 'http' } }),
);
