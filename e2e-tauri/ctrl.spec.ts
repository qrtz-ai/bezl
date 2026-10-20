import { expect, test, type Page } from '@playwright/test';
import { axe } from './axe';

// ctrl do (ADR 0214) in the shipped renderer over the mocked Tauri IPC: type one command, watch the run's decisions
// and actions, answer the confirm card or the ask card, and scrub the run. The chain itself runs over fixture trees
// in `cargo test -p qrtz-ctrl`; the mock replays canned runs of the same shape.
async function open(page: Page, errors: string[], withModel = true) {
  page.on('pageerror', (e) => errors.push(e.message));
  page.on('console', (m) => {
    if (m.type() === 'error') errors.push(m.text());
  });
  await page.goto('/?dev=1');
  await page.getByTestId('nav-ctrl').click();
  const ctrl = page.locator('[data-subapp="ctrl"]');
  await expect(ctrl.getByTestId('ctrl')).toBeVisible();
  // The decision model is required: the mock starts without it, so a yes downloads it and lifts the gate.
  if (withModel) {
    await ctrl.getByTestId('ctrl-model-on').click();
    await expect(ctrl.getByTestId('ctrl-model-gate')).toHaveCount(0);
  }
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

test('ctrl: without the decision model the page is blocked; a yes installs it, off blocks again', async ({
  page,
}) => {
  const errors: string[] = [];
  const ctrl = await open(page, errors, false);
  const gate = ctrl.getByTestId('ctrl-model-gate');
  const banner = ctrl.getByTestId('ctrl-model');
  await expect(banner).toHaveAttribute('data-state', 'missing');
  await expect(banner.getByTestId('ctrl-model-title')).toHaveText(
    'ctrl needs the decision model',
  );
  await expect(banner.getByTestId('ctrl-model-line')).toHaveText(
    'not downloaded · 484 mb',
  );
  await expect(ctrl.getByText('built-in rules')).toHaveCount(0);
  await expect(gate.getByTestId('ctrl-model-blocked')).toHaveAttribute(
    'inert',
    '',
  );
  // Inert: the do field sits under the blocked, blurred body and can't take focus.
  expect(
    await ctrl
      .getByTestId('ctrl-do-input')
      .evaluate((el) => !!el.closest('[inert]')),
  ).toBe(true);
  await expect(banner.getByTestId('ctrl-model-on')).toHaveText('download');
  await banner.getByTestId('ctrl-model-on').click();
  await expect(gate).toHaveCount(0);
  await expect(ctrl.getByTestId('ctrl-do-input')).toBeEditable();
  const card = ctrl.getByTestId('ctrl-model');
  await expect(card).toHaveAttribute('data-state', 'ready');
  await card.getByTestId('ctrl-model-off').click();
  await expect(gate).toBeVisible();
  expect(errors).toEqual([]);
});
