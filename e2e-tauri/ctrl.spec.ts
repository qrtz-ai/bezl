import { expect, test, type Page } from '@playwright/test';
import { axe } from './axe';

// ctrl do (ADR 0214) in the shipped renderer over the mocked Tauri IPC: type one command, watch the run's decisions
// and actions, answer the confirm card or the ask card, and scrub the run. The chain itself runs over fixture trees
// in `cargo test -p qrtz-ctrl`; the mock replays canned runs of the same shape.
async function open(page: Page, errors: string[]) {
  page.on('pageerror', (e) => errors.push(e.message));
  page.on('console', (m) => {
    if (m.type() === 'error') errors.push(m.text());
  });
  await page.goto('/?dev=1');
  await page.getByTestId('nav-ctrl').click();
  const ctrl = page.locator('[data-subapp="ctrl"]');
  await expect(ctrl.getByTestId('ctrl')).toBeVisible();
  return ctrl;
}

async function doIt(ctrl: ReturnType<Page['locator']>, text: string) {
  await ctrl.getByTestId('ctrl-do-input').fill(text);
  await ctrl.getByTestId('ctrl-do-go').click();
}

test('ctrl do: open youtube runs through safari, lists it and scrubs back', async ({
  page,
}) => {
  const errors: string[] = [];
  const ctrl = await open(page, errors);
  await expect(ctrl.getByText('nothing done yet')).toBeVisible();
  await expect(ctrl.getByTestId('ctrl-do-go')).toBeDisabled();
  await doIt(ctrl, 'open youtube');
  const run = ctrl.getByTestId('ctrl-current');
  await expect(run).toBeVisible();
  await expect(ctrl.getByTestId('ctrl-run-status').first()).toContainText(
    'done',
  );
  await expect(run.getByTestId('ctrl-decision').first()).toBeVisible();
  await expect(ctrl.getByTestId('ctrl-stopbar')).toHaveCount(0);
  await expect(ctrl.getByTestId('ctrl-do-input')).toHaveValue('');
  expect(await axe(page, 'ctrl')).toEqual([]);
  expect(errors).toEqual([]);
});

test('ctrl do: a send waits at the confirm card; deny cancels, approve finishes', async ({
  page,
}) => {
  const errors: string[] = [];
  const ctrl = await open(page, errors);
  await doIt(ctrl, 'press send in mail');
  const card = ctrl.getByTestId('ctrl-confirm');
  await expect(card).toBeVisible();
  await expect(card.getByTestId('ctrl-confirm-action')).toContainText(
    'press send',
  );
  await expect(ctrl.getByTestId('ctrl-run-status').first()).toContainText(
    'needs your ok',
  );
  await card.getByTestId('ctrl-confirm-deny').click();
  await expect(card).toHaveCount(0);
  await expect(ctrl.getByTestId('ctrl-run-status').first()).toContainText(
    'cancelled',
  );

  await doIt(ctrl, 'press send in mail');
  await ctrl.getByTestId('ctrl-confirm-approve').click();
  await expect(ctrl.getByTestId('ctrl-confirm')).toHaveCount(0);
  await expect(ctrl.getByTestId('ctrl-run-status').first()).toContainText(
    'done',
  );
  await expect(ctrl.getByTestId('ctrl-run-status')).toHaveCount(2);
  expect(errors).toEqual([]);
});

test('ctrl do: an unclear command asks for one of three; a pick finishes, cancel ends it', async ({
  page,
}) => {
  const errors: string[] = [];
  const ctrl = await open(page, errors);
  await doIt(ctrl, 'tidy up slack');
  const ask = ctrl.getByTestId('ctrl-ask');
  await expect(ask).toBeVisible();
  await expect(ask.getByTestId('ctrl-ask-option')).toHaveCount(3);
  expect(await axe(page, 'ctrl')).toEqual([]);
  await ask.getByTestId('ctrl-ask-option').first().click();
  await expect(ask).toHaveCount(0);
  await expect(ctrl.getByTestId('ctrl-run-status').first()).toContainText(
    'done',
  );

  await doIt(ctrl, 'tidy up slack');
  await ctrl.getByTestId('ctrl-ask-cancel').click();
  await expect(ctrl.getByTestId('ctrl-ask')).toHaveCount(0);
  await expect(ctrl.getByTestId('ctrl-run-status').first()).toContainText(
    'cancelled',
  );
  expect(errors).toEqual([]);
});

test('ctrl: the decision model is a choice; a yes installs the pinned model, off removes it', async ({
  page,
}) => {
  const errors: string[] = [];
  const ctrl = await open(page, errors);
  const card = ctrl.getByTestId('ctrl-model');
  await expect(card).toHaveAttribute('data-state', 'off');
  await expect(card.getByTestId('ctrl-model-scorer')).toHaveText(
    'deciding with the built-in rules',
  );
  await expect(card.getByTestId('ctrl-model-on')).toContainText('(484 mb)');
  await card.getByTestId('ctrl-model-on').click();
  await expect(card).toHaveAttribute('data-state', 'installed');
  await card.getByTestId('ctrl-model-off').click();
  await expect(card).toHaveAttribute('data-state', 'off');
  expect(errors).toEqual([]);
});
