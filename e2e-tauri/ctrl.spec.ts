import { expect, test } from '@playwright/test';
import { axe } from './axe';

// ctrl (ADR 0103) in the shipped renderer over the mocked Tauri IPC: type a task, pick a schedule, see it listed,
// run it now, see its run history (derived from the run's event log) and scrub the run's state.
test('ctrl: create a task, see it listed, run it, see and scrub its history', async ({
  page,
}) => {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(e.message));
  page.on('console', (m) => {
    if (m.type() === 'error') errors.push(m.text());
  });
  await page.goto('/?dev=1');
  await page.getByTestId('nav-ctrl').click();
  const ctrl = page.locator('[data-subapp="ctrl"]');
  await expect(ctrl.getByTestId('ctrl')).toBeVisible();
  await expect(ctrl.getByText('no tasks yet')).toBeVisible();

  const add = ctrl.getByTestId('ctrl-task-add');
  await expect(add).toBeDisabled();
  await ctrl
    .getByTestId('ctrl-task-input')
    .fill('every week clean up my downloads folder');
  await ctrl.getByTestId('ctrl-schedule-weekly').click();
  await ctrl.getByTestId('ctrl-schedule-day').selectOption('1');
  await ctrl.getByTestId('ctrl-schedule-time').fill('18:30');
  await add.click();

  const row = ctrl.getByTestId('ctrl-task');
  await expect(row).toHaveCount(1);
  await expect(row.getByTestId('ctrl-task-text')).toHaveText(
    'every week clean up my downloads folder',
  );
  await expect(row.getByTestId('ctrl-task-schedule')).toHaveText(
    'every monday at 18:30 · never run',
  );
  await expect(ctrl.getByTestId('ctrl-task-input')).toHaveValue('');
  await expect(ctrl.getByText('no runs yet')).toBeVisible();

  // A second task, once / as soon as possible: newest first.
  await ctrl.getByTestId('ctrl-task-input').fill('every day close all my tabs');
  await ctrl.getByTestId('ctrl-schedule-daily').click();
  await ctrl.getByTestId('ctrl-schedule-time').fill('09:00');
  await add.click();
  await expect(row).toHaveCount(2);
  await expect(row.first().getByTestId('ctrl-task-text')).toHaveText(
    'every day close all my tabs',
  );

  const downloads = row.filter({ hasText: 'downloads' });
  await downloads.getByTestId('ctrl-task-run').click();
  const runs = ctrl.getByTestId('ctrl-run');
  await expect(runs).toHaveCount(1);
  await expect(runs.first().getByTestId('ctrl-run-status')).toHaveText(
    'waiting for intent',
  );
  await expect(downloads.getByTestId('ctrl-task-schedule')).toContainText(
    'last run',
  );
  await expect(ctrl.getByTestId('ctrl-event')).toHaveCount(2);
  await expect(ctrl.getByTestId('ctrl-state')).toHaveText(
    'step 2 of 2 · waiting for intent',
  );
  await ctrl.getByTestId('ctrl-scrub').fill('1');
  await expect(ctrl.getByTestId('ctrl-state')).toHaveText(
    'step 1 of 2 · running',
  );
  await expect(
    ctrl.locator('[data-testid="ctrl-event"][data-applied]'),
  ).toHaveCount(1);

  await downloads.getByTestId('ctrl-task-run').click();
  await expect(runs).toHaveCount(2);

  await downloads.getByTestId('ctrl-task-pause').click();
  await expect(downloads.getByTestId('ctrl-task-schedule')).toContainText(
    'paused · every monday at 18:30',
  );
  expect(await axe(page, 'ctrl')).toEqual([]);

  await row.first().getByTestId('ctrl-task-delete').click();
  await expect(row).toHaveCount(1);
  expect(errors).toEqual([]);
});
