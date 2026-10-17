export { fmtClock } from '@qrtz/core';

export const PLATFORM_LABEL: Record<string, string> = {
  zoom: 'Zoom',
  'google-meet': 'Google Meet',
  teams: 'Teams',
  'slack-huddle': 'Slack',
  webex: 'Webex',
  other: 'Recording',
};

export const fmtDate = (ms: number) =>
  new Date(ms).toLocaleString(undefined, {
    month: 'short',
    day: 'numeric',
    hour: 'numeric',
    minute: '2-digit',
  });
