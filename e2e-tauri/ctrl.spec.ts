import { expect, test } from '@playwright/test';
import { axe } from './axe';

// ctrl (ADR 0103, 0106) in the shipped renderer over the mocked Tauri IPC: type a task, pick a schedule, see it
// listed, run it now, see its run history (derived from the run's event log), the decision layers' plan and
// inspector, scrub the run's state, and answer a run that asks.
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
  await expect(runs.first().getByTestId('ctrl-run-status')).toHaveText('done');
  await expect(downloads.getByTestId('ctrl-task-schedule')).toContainText(
    'last run',
  );
  // task.started, intent.pending, obs.apps, 4 decisions, plan.produced, then one guarded batch of file moves
  // (slice 7, ADR 0113: no gate, nothing deleted), verify.result, task.finished.
  await expect(ctrl.getByTestId('ctrl-event')).toHaveCount(16);
  await expect(ctrl.getByTestId('ctrl-state')).toHaveText(
    'step 16 of 16 · done · 4 decisions · 1/1 actions done · 1/1 verified',
  );
  await expect(ctrl.getByTestId('ctrl-confirm')).toHaveCount(0);
  await expect(ctrl.getByTestId('ctrl-plan-steps')).toHaveText(
    'plan: clean up a folder → files (no app) → downloads → sort by kind, nothing deleted',
  );
  const decisions = ctrl.getByTestId('ctrl-decision');
  await expect(decisions).toHaveCount(4);
  await expect(
    decisions.nth(2).getByTestId('ctrl-decision-question'),
  ).toHaveText('on what');
  await expect(
    decisions.nth(2).locator('[data-testid="ctrl-option"][data-chosen]'),
  ).toContainText('downloads');
  await expect(
    decisions.first().getByTestId('ctrl-decision-outcome'),
  ).toHaveText('sure');
  // Scrub back to before the decisions: no plan, no inspector, still waiting for intent.
  await ctrl.getByTestId('ctrl-scrub').fill('2');
  await expect(ctrl.getByTestId('ctrl-state')).toHaveText(
    'step 2 of 16 · waiting for intent',
  );
  await expect(ctrl.getByTestId('ctrl-plan')).toHaveCount(0);
  await expect(decisions).toHaveCount(0);
  await ctrl.getByTestId('ctrl-scrub').fill('1');
  await expect(ctrl.getByTestId('ctrl-state')).toHaveText(
    'step 1 of 16 · running',
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

test('ctrl: an unclear task asks which one, the answer plans it, cancel ends a run', async ({
  page,
}) => {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(e.message));
  await page.goto('/?dev=1');
  await page.getByTestId('nav-ctrl').click();
  const ctrl = page.locator('[data-subapp="ctrl"]');
  await ctrl.getByTestId('ctrl-task-input').fill('do the thing');
  await ctrl.getByTestId('ctrl-schedule-once').click();
  await ctrl.getByTestId('ctrl-schedule-time').fill('');
  await ctrl.getByTestId('ctrl-task-add').click();
  const row = ctrl.getByTestId('ctrl-task');
  await row.getByTestId('ctrl-task-run').click();

  const runs = ctrl.getByTestId('ctrl-run');
  await expect(runs.first().getByTestId('ctrl-run-status')).toHaveText(
    'needs you',
  );
  const ask = ctrl.getByTestId('ctrl-ask');
  await expect(ask).toContainText('what kind of task: which one?');
  await expect(ask.getByTestId('ctrl-ask-option')).toHaveCount(3);
  await expect(
    ctrl
      .getByTestId('ctrl-decision')
      .first()
      .getByTestId('ctrl-decision-outcome'),
  ).toHaveText('asked you');
  expect(await axe(page, 'ctrl')).toEqual([]);

  await ask
    .getByTestId('ctrl-ask-option')
    .filter({ hasText: 'close browser tabs' })
    .click();
  await expect(ask).toHaveCount(0);
  // With the helper connected the plan executes in the background and verifies (ADR 0108).
  await expect(runs.first().getByTestId('ctrl-run-status')).toHaveText('done');
  await expect(ctrl.getByTestId('ctrl-plan-steps')).toHaveText(
    'plan: close browser tabs → your default browser → all tabs → close every tab by script',
  );
  await expect(
    ctrl
      .getByTestId('ctrl-decision')
      .first()
      .getByTestId('ctrl-decision-outcome'),
  ).toHaveText('you chose');

  // A second run asks again; cancel ends it.
  await row.getByTestId('ctrl-task-run').click();
  await expect(runs).toHaveCount(2);
  await ctrl.getByTestId('ctrl-ask-cancel').click();
  await expect(runs.first().getByTestId('ctrl-run-status')).toHaveText(
    'cancelled',
  );
  await expect(ask).toHaveCount(0);
  expect(errors).toEqual([]);
});

test('ctrl: a close-tabs run executes in the background, verifies, and scrubs back to the plan', async ({
  page,
}) => {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(e.message));
  await page.goto('/?dev=1');
  await page.getByTestId('nav-ctrl').click();
  const ctrl = page.locator('[data-subapp="ctrl"]');
  await ctrl.getByTestId('ctrl-task-input').fill('every day close all my tabs');
  await ctrl.getByTestId('ctrl-schedule-daily').click();
  await ctrl.getByTestId('ctrl-schedule-time').fill('09:00');
  await ctrl.getByTestId('ctrl-task-add').click();
  await ctrl.getByTestId('ctrl-task').getByTestId('ctrl-task-run').click();

  const runs = ctrl.getByTestId('ctrl-run');
  await expect(runs.first().getByTestId('ctrl-run-status')).toHaveText('done');
  // 3 + 4 decisions + plan.produced + one guarded action (requested, decided, 2 guard snapshots, executed,
  // guard result) + verify.result + task.finished.
  await expect(ctrl.getByTestId('ctrl-event')).toHaveCount(16);
  await expect(ctrl.getByTestId('ctrl-state')).toHaveText(
    'step 16 of 16 · done · 4 decisions · 1/1 actions done · 1/1 verified',
  );
  await expect(
    ctrl.getByTestId('ctrl-event').filter({ hasText: 'verify result' }),
  ).toHaveCount(1);
  expect(await axe(page, 'ctrl')).toEqual([]);
  // Scrub back to the plan, before the action: planned, nothing done yet.
  await ctrl.getByTestId('ctrl-scrub').fill('8');
  await expect(ctrl.getByTestId('ctrl-state')).toHaveText(
    'step 8 of 16 · planned · 4 decisions',
  );
  await expect(ctrl.getByTestId('ctrl-plan-steps')).toHaveText(
    'plan: close browser tabs → your default browser → all tabs → close every tab by script',
  );
  // The rows after the scrub position are muted, and still pass axe's contrast check (run 5's finding).
  await expect(
    ctrl.locator('[data-testid="ctrl-event"]:not([data-applied])'),
  ).toHaveCount(8);
  expect(await axe(page, 'ctrl')).toEqual([]);
  expect(errors).toEqual([]);
});

// The confirm gate (ADR 0111): an irreversible task stops with a card in the ctrl view (never a modal) that lists
// exactly what will happen; "don't" ends the run with nothing done, "do it once" acts once, "don't ask again"
// leaves a standing approval the next run uses without asking, and revoking it brings the card back.
test('ctrl: the confirm card lists what will happen; deny, approve and remember', async ({
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
  await ctrl.getByTestId('ctrl-task-input').fill('every day quit textedit');
  await ctrl.getByTestId('ctrl-schedule-daily').click();
  await ctrl.getByTestId('ctrl-schedule-time').fill('18:00');
  await ctrl.getByTestId('ctrl-task-add').click();
  const row = ctrl.getByTestId('ctrl-task');
  const runs = ctrl.getByTestId('ctrl-run');
  const card = ctrl.getByTestId('ctrl-confirm');

  // Deny: nothing is done.
  await row.getByTestId('ctrl-task-run').click();
  await expect(runs.first().getByTestId('ctrl-run-status')).toHaveText(
    'needs your ok',
  );
  await expect(card).toBeVisible();
  await expect(card).toContainText(
    "quit an app · textedit: this can't be undone",
  );
  await expect(card.getByTestId('ctrl-confirm-item')).toHaveText([
    'notes.txt',
    'draft.txt · unsaved changes',
  ]);
  await expect(card.getByTestId('ctrl-confirm-unsaved')).toBeVisible();
  // A card, not a modal: nothing on the page is inert or trapped behind a dialog.
  await expect(page.getByRole('dialog')).toHaveCount(0);
  expect(await axe(page, 'ctrl')).toEqual([]);
  await card.getByTestId('ctrl-confirm-deny').click();
  await expect(card).toHaveCount(0);
  await expect(ctrl.getByTestId('ctrl-confirm-outcome')).toHaveText(
    'you said no; nothing was done',
  );
  await expect(runs.first().getByTestId('ctrl-run-status')).toHaveText(
    'cancelled',
  );

  // Approve once: the run acts and finishes.
  await row.getByTestId('ctrl-task-run').click();
  await expect(runs).toHaveCount(2);
  await card.getByTestId('ctrl-confirm-approve').click();
  await expect(runs.first().getByTestId('ctrl-run-status')).toHaveText('done');
  await expect(ctrl.getByTestId('ctrl-confirm-outcome')).toHaveText(
    'you said yes, once',
  );
  await expect(ctrl.getByTestId('ctrl-standing')).toHaveCount(0);

  // Remember: a standing approval, and the next run doesn't ask.
  await row.getByTestId('ctrl-task-run').click();
  await card.getByTestId('ctrl-confirm-remember').click();
  await expect(runs.first().getByTestId('ctrl-run-status')).toHaveText('done');
  const standing = ctrl.getByTestId('ctrl-approval');
  await expect(standing).toHaveText([
    /quit app in textedit, up to 2 items, 1 unsaved/,
  ]);
  await row.getByTestId('ctrl-task-run').click();
  await expect(runs).toHaveCount(4);
  await expect(runs.first().getByTestId('ctrl-run-status')).toHaveText('done');
  await expect(card).toHaveCount(0);
  await expect(ctrl.getByTestId('ctrl-confirm-outcome')).toHaveText(
    'allowed by your standing ok',
  );
  expect(await axe(page, 'ctrl')).toEqual([]);

  // Revoke: the card is back.
  await standing.getByTestId('ctrl-approval-revoke').click();
  await expect(ctrl.getByTestId('ctrl-standing')).toHaveCount(0);
  await row.getByTestId('ctrl-task-run').click();
  await expect(runs).toHaveCount(5);
  await expect(card).toBeVisible();
  expect(errors).toEqual([]);
});

// Slice 7 (ADR 0113): trashing old installers is the one downloads move that asks. The card lists the files (a
// card, never a modal), approve finishes the run, and the run history shows done.
test('ctrl: trashing old downloads waits at the card with the files, approve finishes it', async ({
  page,
}) => {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(e.message));
  await page.goto('/?dev=1');
  await page.getByTestId('nav-ctrl').click();
  const ctrl = page.locator('[data-subapp="ctrl"]');
  await ctrl
    .getByTestId('ctrl-task-input')
    .fill('every week delete old installers from my downloads');
  await ctrl.getByTestId('ctrl-schedule-weekly').click();
  await ctrl.getByTestId('ctrl-schedule-time').fill('18:00');
  await ctrl.getByTestId('ctrl-task-add').click();
  await ctrl.getByTestId('ctrl-task').getByTestId('ctrl-task-run').click();
  const runs = ctrl.getByTestId('ctrl-run');
  const card = ctrl.getByTestId('ctrl-confirm');
  await expect(runs.first().getByTestId('ctrl-run-status')).toHaveText(
    'needs your ok',
  );
  await expect(card.getByTestId('ctrl-confirm-item')).toHaveText([
    'ancient.pkg',
    'oldapp.dmg',
  ]);
  await expect(card.getByTestId('ctrl-confirm-unsaved')).toHaveCount(0);
  await expect(page.getByRole('dialog')).toHaveCount(0);
  expect(await axe(page, 'ctrl')).toEqual([]);
  await card.getByTestId('ctrl-confirm-approve').click();
  await expect(runs.first().getByTestId('ctrl-run-status')).toHaveText('done');
  await expect(ctrl.getByTestId('ctrl-confirm-outcome')).toHaveText(
    'you said yes, once',
  );
  expect(await axe(page, 'ctrl')).toEqual([]);
  expect(errors).toEqual([]);
});

// ADR 0122: emptying the Trash can't be undone, so even a weekly run waits at the card, which lists what goes (the
// Finder's own files and anything just trashed are left out); no put back afterwards. Deny leaves it all there.
test('ctrl: every friday empty the trash waits at the card, deny keeps it, approve empties', async ({
  page,
}) => {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(e.message));
  await page.goto('/?dev=1');
  await page.getByTestId('nav-ctrl').click();
  const ctrl = page.locator('[data-subapp="ctrl"]');
  await ctrl
    .getByTestId('ctrl-task-input')
    .fill('every friday empty the trash');
  await ctrl.getByTestId('ctrl-schedule-weekly').click();
  await ctrl.getByTestId('ctrl-schedule-time').fill('18:00');
  await ctrl.getByTestId('ctrl-task-add').click();
  const row = ctrl.getByTestId('ctrl-task');
  await row.getByTestId('ctrl-task-run').click();
  const runs = ctrl.getByTestId('ctrl-run');
  const card = ctrl.getByTestId('ctrl-confirm');
  await expect(runs.first().getByTestId('ctrl-run-status')).toHaveText(
    'needs your ok',
  );
  await expect(card).toContainText(
    "empty the trash · files (no app): this can't be undone",
  );
  await expect(card.getByTestId('ctrl-confirm-item')).toHaveText([
    'old folder',
    'old.dmg',
  ]);
  await expect(page.getByRole('dialog')).toHaveCount(0);
  expect(await axe(page, 'ctrl')).toEqual([]);
  await card.getByTestId('ctrl-confirm-deny').click();
  await expect(runs.first().getByTestId('ctrl-run-status')).not.toHaveText(
    'done',
  );
  await row.getByTestId('ctrl-task-run').click();
  await expect(runs).toHaveCount(2);
  await expect(card.getByTestId('ctrl-confirm-item')).toHaveCount(2);
  await card.getByTestId('ctrl-confirm-approve').click();
  await expect(runs.first().getByTestId('ctrl-run-status')).toHaveText('done');
  await expect(ctrl.getByTestId('ctrl-undo')).toHaveCount(0);
  expect(await axe(page, 'ctrl')).toEqual([]);
  expect(errors).toEqual([]);
});

// ADR 0119: when the browser blocks ctrl's read-only page script every held tab reads "can't tell"; the card says
// why, in plain lowercase text, and the answers are the same.
test('ctrl: the tabs card says when ctrl could not look inside the tabs', async ({
  page,
}) => {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(e.message));
  await page.goto('/?dev=1');
  await page.getByTestId('nav-ctrl').click();
  const ctrl = page.locator('[data-subapp="ctrl"]');
  await ctrl
    .getByTestId('ctrl-task-input')
    .fill('close all my tabs, scripts blocked');
  await ctrl.getByTestId('ctrl-task-add').click();
  await ctrl.getByTestId('ctrl-task').getByTestId('ctrl-task-run').click();
  const card = ctrl.getByTestId('ctrl-confirm');
  await expect(card.getByTestId('ctrl-confirm-item')).toHaveText([
    'inbox · may have unsaved changes',
    'expense form · may have unsaved changes',
  ]);
  const hint = card.getByTestId('ctrl-confirm-tabs-hint');
  await expect(hint).toContainText('allow javascript from apple events');
  expect(await hint.textContent()).toBe(
    (await hint.textContent())!.toLowerCase(),
  );
  expect(await axe(page, 'ctrl')).toEqual([]);
  await card.getByTestId('ctrl-confirm-deny').click();
  await expect(card).toHaveCount(0);
  expect(errors).toEqual([]);
});

// ADR 0116 / 0129: the decision model is off until the user says yes and names its size (the pinned laya-coreml
// bucket + tokenizer, 484 mb); a yes installs it (the mock fakes the checked download), the browser has no core ml
// so the built-in rules still decide; turning it off deletes it and goes back to off.
test('ctrl: the decision model is a choice; a yes installs the pinned model, off removes it', async ({
  page,
}) => {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(e.message));
  await page.goto('/?dev=1');
  await page.getByTestId('nav-ctrl').click();
  const ctrl = page.locator('[data-subapp="ctrl"]');
  const card = ctrl.getByTestId('ctrl-model');
  await expect(card).toHaveAttribute('data-state', 'off');
  await expect(card.getByTestId('ctrl-model-scorer')).toHaveText(
    'deciding with the built-in rules',
  );
  await expect(card.getByTestId('ctrl-model-on')).toContainText('(484 mb)');
  await card.getByTestId('ctrl-model-on').click();
  await expect(card).toHaveAttribute('data-state', 'installed');
  await expect(card.getByTestId('ctrl-model-line')).toHaveText(
    'downloaded and checked; runs on this mac only',
  );
  await expect(card.getByTestId('ctrl-model-scorer')).toHaveText(
    'deciding with the built-in rules',
  );
  expect(await axe(page, 'ctrl')).toEqual([]);
  await card.getByTestId('ctrl-model-off').click();
  await expect(card).toHaveAttribute('data-state', 'off');
  await expect(card.getByTestId('ctrl-model-on')).toBeVisible();
  expect(errors).toEqual([]);
});

// ADR 0117: a finished downloads cleanup can be put back from its run. Nothing changed: one press puts every file
// back. A file renamed since: the card says which file stays, offers only the others, and the put-back is logged.
test('ctrl: put a finished downloads cleanup back; a renamed file stays and is named', async ({
  page,
}) => {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(e.message));
  await page.goto('/?dev=1');
  await page.getByTestId('nav-ctrl').click();
  const ctrl = page.locator('[data-subapp="ctrl"]');
  const add = async (text: string) => {
    await ctrl.getByTestId('ctrl-task-input').fill(text);
    await ctrl.getByTestId('ctrl-schedule-weekly').click();
    await ctrl.getByTestId('ctrl-schedule-time').fill('18:00');
    await ctrl.getByTestId('ctrl-task-add').click();
  };
  const runs = ctrl.getByTestId('ctrl-run');
  const card = ctrl.getByTestId('ctrl-undo');

  await add('every week clean up my downloads folder');
  await ctrl.getByTestId('ctrl-task').getByTestId('ctrl-task-run').click();
  await expect(runs.first().getByTestId('ctrl-run-status')).toHaveText('done');
  await expect(card.getByTestId('ctrl-undo-title')).toHaveText(
    'ctrl moved 3 files',
  );
  await expect(card.getByTestId('ctrl-undo-rest')).toHaveCount(0);
  await expect(page.getByRole('dialog')).toHaveCount(0);
  expect(await axe(page, 'ctrl')).toEqual([]);
  await card.getByTestId('ctrl-undo-all').click();
  await expect(card.getByTestId('ctrl-undo-title')).toHaveText(
    'put back 3 files',
  );
  await expect(card.getByRole('button')).toHaveCount(0);
  await expect(ctrl.getByTestId('ctrl-event').last()).toContainText('undo');

  await add('every week clean up my downloads folder, photo renamed');
  await ctrl
    .getByTestId('ctrl-task')
    .filter({ hasText: 'renamed' })
    .getByTestId('ctrl-task-run')
    .click();
  await expect(runs.first().getByTestId('ctrl-run-status')).toHaveText('done');
  await expect(card.getByTestId('ctrl-undo-cant-item')).toHaveText([
    'photo.png · moved or renamed since',
  ]);
  await expect(card.getByTestId('ctrl-undo-all')).toHaveCount(0);
  expect(await axe(page, 'ctrl')).toEqual([]);
  await card.getByTestId('ctrl-undo-rest').click();
  await expect(card.getByTestId('ctrl-undo-title')).toHaveText(
    'put back 2 files',
  );
  await expect(card).toContainText('1 file stayed put');
  expect(await axe(page, 'ctrl')).toEqual([]);
  expect(errors).toEqual([]);
});

// ADR 0125: ctrl has no routes for the Documents folder yet. The run stops and says so in plain lowercase text;
// it doesn't ask a question no answer could settle, and nothing moves (no put back offered).
test('ctrl: tidy my documents stops and says it cannot tidy that folder yet', async ({
  page,
}) => {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(e.message));
  await page.goto('/?dev=1');
  await page.getByTestId('nav-ctrl').click();
  const ctrl = page.locator('[data-subapp="ctrl"]');
  await ctrl
    .getByTestId('ctrl-task-input')
    .fill('every week tidy my documents');
  await ctrl.getByTestId('ctrl-schedule-weekly').click();
  await ctrl.getByTestId('ctrl-schedule-time').fill('18:00');
  await ctrl.getByTestId('ctrl-task-add').click();
  await ctrl.getByTestId('ctrl-task').getByTestId('ctrl-task-run').click();
  const run = ctrl.getByTestId('ctrl-run').first();
  await expect(run.getByTestId('ctrl-run-status')).toHaveText('needs you');
  await expect(ctrl.getByTestId('ctrl-state').first()).toContainText(
    "ctrl can't tidy this folder yet, nothing moved",
  );
  await expect(ctrl.getByTestId('ctrl-confirm')).toHaveCount(0);
  await expect(ctrl.getByTestId('ctrl-undo')).toHaveCount(0);
  await expect(page.getByRole('dialog')).toHaveCount(0);
  expect(await axe(page, 'ctrl')).toEqual([]);
  expect(errors).toEqual([]);
});

// ADR 0148: "quit <app>" quits one named app through the workspace, never the app the user is in. Quitting
// textedit (in the background) waits at the card; "quit notes" (the frontmost app) stops before any card and says
// it has no background way, and nothing is quit.
test('ctrl: quit an app waits at the card, and the app you are in is never quit', async ({
  page,
}) => {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(e.message));
  await page.goto('/?dev=1');
  await page.getByTestId('nav-ctrl').click();
  const ctrl = page.locator('[data-subapp="ctrl"]');
  await ctrl.getByTestId('ctrl-task-input').fill('quit textedit');
  await ctrl.getByTestId('ctrl-task-add').click();
  await ctrl.getByTestId('ctrl-task').getByTestId('ctrl-task-run').click();
  const card = ctrl.getByTestId('ctrl-confirm');
  await expect(card).toBeVisible();
  await expect(card).toContainText(
    "quit an app · textedit: this can't be undone",
  );
  await card.getByTestId('ctrl-confirm-approve').click();
  await expect(
    ctrl.getByTestId('ctrl-run').first().getByTestId('ctrl-run-status'),
  ).toHaveText('done');

  await ctrl.getByTestId('ctrl-task-input').fill('quit notes');
  await ctrl.getByTestId('ctrl-task-add').click();
  const notes = ctrl.getByTestId('ctrl-task').filter({ hasText: 'quit notes' });
  await notes.getByTestId('ctrl-task-run').click();
  const run = ctrl.getByTestId('ctrl-run').first();
  await expect(run.getByTestId('ctrl-run-status')).toHaveText('needs you');
  await expect(ctrl.getByTestId('ctrl-state').first()).toContainText(
    'no way to do it in the background',
  );
  await expect(card).toHaveCount(0);
  await expect(page.getByRole('dialog')).toHaveCount(0);
  expect(await axe(page, 'ctrl')).toEqual([]);
  expect(errors).toEqual([]);
});

// ADR 0124 / 0126: "tidy my desktop" moves only macOS's screenshots and screen recordings into their folder, in
// one batch with no card (nothing is deleted), and the run can be put back from its card.
test('ctrl: tidy my desktop moves the screenshots without a card and puts them back', async ({
  page,
}) => {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(e.message));
  await page.goto('/?dev=1');
  await page.getByTestId('nav-ctrl').click();
  const ctrl = page.locator('[data-subapp="ctrl"]');
  await ctrl.getByTestId('ctrl-task-input').fill('every week tidy my desktop');
  await ctrl.getByTestId('ctrl-schedule-weekly').click();
  await ctrl.getByTestId('ctrl-schedule-time').fill('18:00');
  await ctrl.getByTestId('ctrl-task-add').click();
  await ctrl.getByTestId('ctrl-task').getByTestId('ctrl-task-run').click();
  const run = ctrl.getByTestId('ctrl-run').first();
  await expect(run.getByTestId('ctrl-run-status')).toHaveText('done');
  await expect(ctrl.getByTestId('ctrl-confirm')).toHaveCount(0);
  await expect(ctrl.getByTestId('ctrl-decisions').first()).toContainText(
    'move screenshots into a folder, nothing deleted',
  );
  const card = ctrl.getByTestId('ctrl-undo');
  await expect(card.getByTestId('ctrl-undo-title')).toHaveText(
    'ctrl moved 2 files',
  );
  await expect(page.getByRole('dialog')).toHaveCount(0);
  expect(await axe(page, 'ctrl')).toEqual([]);
  await card.getByTestId('ctrl-undo-all').click();
  await expect(card.getByTestId('ctrl-undo-title')).toHaveText(
    'put back 2 files',
  );
  expect(await axe(page, 'ctrl')).toEqual([]);
  expect(errors).toEqual([]);
});

// ADR 0128: "every morning open my calendar" picks calendar.app and opens it without bringing it forward; the run
// is done with no card and no dialog.
test('ctrl: every morning open my calendar opens calendar without a card', async ({
  page,
}) => {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(e.message));
  await page.goto('/?dev=1');
  await page.getByTestId('nav-ctrl').click();
  const ctrl = page.locator('[data-subapp="ctrl"]');
  await ctrl
    .getByTestId('ctrl-task-input')
    .fill('every morning open my calendar');
  await ctrl.getByTestId('ctrl-schedule-daily').click();
  await ctrl.getByTestId('ctrl-schedule-time').fill('08:00');
  await ctrl.getByTestId('ctrl-task-add').click();
  await ctrl.getByTestId('ctrl-task').getByTestId('ctrl-task-run').click();
  const run = ctrl.getByTestId('ctrl-run').first();
  await expect(run.getByTestId('ctrl-run-status')).toHaveText('done');
  await expect(ctrl.getByTestId('ctrl-confirm')).toHaveCount(0);
  const decisions = ctrl.getByTestId('ctrl-decisions').first();
  await expect(decisions).toContainText('open an app');
  await expect(decisions).toContainText('calendar');
  await expect(decisions).toContainText('start it hidden');
  await expect(page.getByRole('dialog')).toHaveCount(0);
  expect(await axe(page, 'ctrl')).toEqual([]);
  expect(errors).toEqual([]);
});

// ADR 0130: "quit apps i haven't used today" lists the apps it would quit on the card (never the frontmost app,
// finder or qrtz), deny keeps them running, approve quits them once.
test('ctrl: quit apps i have not used today waits at the card with the apps', async ({
  page,
}) => {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(e.message));
  await page.goto('/?dev=1');
  await page.getByTestId('nav-ctrl').click();
  const ctrl = page.locator('[data-subapp="ctrl"]');
  await ctrl
    .getByTestId('ctrl-task-input')
    .fill("quit apps i haven't used today");
  await ctrl.getByTestId('ctrl-task-add').click();
  const row = ctrl.getByTestId('ctrl-task');
  await row.getByTestId('ctrl-task-run').click();
  const runs = ctrl.getByTestId('ctrl-run');
  const card = ctrl.getByTestId('ctrl-confirm');
  await expect(runs.first().getByTestId('ctrl-run-status')).toHaveText(
    'needs your ok',
  );
  await expect(card).toContainText(
    "quit apps you haven't used today: this can't be undone",
  );
  await expect(card.getByTestId('ctrl-confirm-item')).toHaveText([
    'slack · may have unsaved changes',
    'textedit · unsaved changes',
  ]);
  await expect(page.getByRole('dialog')).toHaveCount(0);
  expect(await axe(page, 'ctrl')).toEqual([]);
  await card.getByTestId('ctrl-confirm-deny').click();
  await expect(runs.first().getByTestId('ctrl-run-status')).not.toHaveText(
    'done',
  );
  await row.getByTestId('ctrl-task-run').click();
  await expect(runs).toHaveCount(2);
  await card.getByTestId('ctrl-confirm-approve').click();
  await expect(runs.first().getByTestId('ctrl-run-status')).toHaveText('done');
  expect(await axe(page, 'ctrl')).toEqual([]);
  expect(errors).toEqual([]);
});

// ADR 0134: "hide all my apps" hides every other app without a card (reversible), the app you're in stays.
test('ctrl: every morning hide all my apps hides the others without a card', async ({
  page,
}) => {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(e.message));
  await page.goto('/?dev=1');
  await page.getByTestId('nav-ctrl').click();
  const ctrl = page.locator('[data-subapp="ctrl"]');
  await ctrl
    .getByTestId('ctrl-task-input')
    .fill('every morning hide all my apps');
  await ctrl.getByTestId('ctrl-schedule-daily').click();
  await ctrl.getByTestId('ctrl-schedule-time').fill('09:00');
  await ctrl.getByTestId('ctrl-task-add').click();
  await ctrl.getByTestId('ctrl-task').getByTestId('ctrl-task-run').click();
  const run = ctrl.getByTestId('ctrl-run').first();
  await expect(run.getByTestId('ctrl-run-status')).toHaveText('done');
  await expect(ctrl.getByTestId('ctrl-confirm')).toHaveCount(0);
  const decisions = ctrl.getByTestId('ctrl-decisions').first();
  await expect(decisions).toContainText('hide your other apps');
  await expect(decisions).toContainText("hide them, the app you're in stays");
  await expect(page.getByRole('dialog')).toHaveCount(0);
  expect(await axe(page, 'ctrl')).toEqual([]);
  expect(errors).toEqual([]);
});

// ADR 0149: "hide slack" hides that one app without a card; the app you're in ("hide notes") is never hidden.
test('ctrl: hide slack hides only slack without a card, never the app you are in', async ({
  page,
}) => {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(e.message));
  await page.goto('/?dev=1');
  await page.getByTestId('nav-ctrl').click();
  const ctrl = page.locator('[data-subapp="ctrl"]');
  await ctrl.getByTestId('ctrl-task-input').fill('hide slack');
  await ctrl.getByTestId('ctrl-task-add').click();
  await ctrl.getByTestId('ctrl-task').getByTestId('ctrl-task-run').click();
  const run = ctrl.getByTestId('ctrl-run').first();
  await expect(run.getByTestId('ctrl-run-status')).toHaveText('done');
  await expect(ctrl.getByTestId('ctrl-confirm')).toHaveCount(0);
  const decisions = ctrl.getByTestId('ctrl-decisions').first();
  await expect(decisions).toContainText('hide an app');
  await expect(decisions).toContainText('hide it, nothing comes forward');
  await expect(decisions).not.toContainText('hide your other apps');

  await ctrl.getByTestId('ctrl-task-input').fill('hide notes');
  await ctrl.getByTestId('ctrl-task-add').click();
  const notes = ctrl.getByTestId('ctrl-task').filter({ hasText: 'hide notes' });
  await notes.getByTestId('ctrl-task-run').click();
  await expect(
    ctrl.getByTestId('ctrl-run').first().getByTestId('ctrl-run-status'),
  ).toHaveText('needs you');
  await expect(ctrl.getByTestId('ctrl-state').first()).toContainText(
    'no way to do it in the background',
  );
  await expect(ctrl.getByTestId('ctrl-confirm')).toHaveCount(0);
  await expect(page.getByRole('dialog')).toHaveCount(0);
  expect(await axe(page, 'ctrl')).toEqual([]);
  expect(errors).toEqual([]);
});

// ADR 0152: "show textedit" unhides that one app without a card; nothing is activated, nothing launched.
test('ctrl: show textedit unhides only textedit without a card', async ({
  page,
}) => {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(e.message));
  await page.goto('/?dev=1');
  await page.getByTestId('nav-ctrl').click();
  const ctrl = page.locator('[data-subapp="ctrl"]');
  await ctrl.getByTestId('ctrl-task-input').fill('show textedit');
  await ctrl.getByTestId('ctrl-task-add').click();
  await ctrl.getByTestId('ctrl-task').getByTestId('ctrl-task-run').click();
  const run = ctrl.getByTestId('ctrl-run').first();
  await expect(run.getByTestId('ctrl-run-status')).toHaveText('done');
  await expect(ctrl.getByTestId('ctrl-confirm')).toHaveCount(0);
  const decisions = ctrl.getByTestId('ctrl-decisions').first();
  await expect(decisions).toContainText('show a hidden app');
  await expect(decisions).toContainText(
    "show its windows, the app you're in stays in front",
  );
  await expect(decisions).not.toContainText('open an app');
  await expect(page.getByRole('dialog')).toHaveCount(0);
  expect(await axe(page, 'ctrl')).toEqual([]);
  expect(errors).toEqual([]);
});

// ADR 0157: "turn on do not disturb" runs the user's one "qrtz Set Focus" shortcut. Before it's made, the run stops
// and ctrl's own view says how to make it, in lowercase; no card, no question, nothing done.
test('ctrl: turn on do not disturb without the shortcut asks to set it up', async ({
  page,
}) => {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(e.message));
  await page.goto('/?dev=1');
  await page.getByTestId('nav-ctrl').click();
  const ctrl = page.locator('[data-subapp="ctrl"]');
  await ctrl.getByTestId('ctrl-task-input').fill('turn on do not disturb');
  await ctrl.getByTestId('ctrl-task-add').click();
  await ctrl.getByTestId('ctrl-task').getByTestId('ctrl-task-run').click();
  const run = ctrl.getByTestId('ctrl-run').first();
  await expect(run.getByTestId('ctrl-run-status')).toHaveText('needs you');
  await expect(ctrl.getByTestId('ctrl-state').first()).toContainText(
    'set up the shortcut',
  );
  const setup = ctrl.getByTestId('ctrl-focus-setup');
  await expect(setup).toContainText('set up the shortcut once');
  await expect(setup).toContainText('name it qrtz Set Focus');
  await expect(setup).toContainText('add the set focus action');
  const decisions = ctrl.getByTestId('ctrl-decisions').first();
  await expect(decisions).toContainText('turn on do not disturb');
  await expect(ctrl.getByTestId('ctrl-confirm')).toHaveCount(0);
  await expect(page.getByRole('dialog')).toHaveCount(0);
  expect(await axe(page, 'ctrl')).toEqual([]);
  expect(errors).toEqual([]);
});

// ADR 0157: denied Automation stops the same way and says "allow automation".
test('ctrl: a focus task with automation denied asks to allow it', async ({
  page,
}) => {
  await page.goto('/?dev=1&mockFocusShortcut=denied');
  await page.getByTestId('nav-ctrl').click();
  const ctrl = page.locator('[data-subapp="ctrl"]');
  await ctrl.getByTestId('ctrl-task-input').fill('turn on do not disturb');
  await ctrl.getByTestId('ctrl-task-add').click();
  await ctrl.getByTestId('ctrl-task').getByTestId('ctrl-task-run').click();
  await expect(ctrl.getByTestId('ctrl-state').first()).toContainText(
    'allow automation',
  );
  await expect(ctrl.getByTestId('ctrl-focus-setup')).toContainText(
    'privacy and security, automation',
  );
});

// ADR 0157: with the shortcut made, focus goes on and off in the background with no card and no setup ask.
for (const [text, move] of [
  ['turn on do not disturb', 'turn on do not disturb'],
  ['turn off do not disturb', 'turn focus off'],
] as const) {
  test(`ctrl: ${text} with the shortcut runs it`, async ({ page }) => {
    const errors: string[] = [];
    page.on('pageerror', (e) => errors.push(e.message));
    await page.goto('/?dev=1&mockFocusShortcut=1');
    await page.getByTestId('nav-ctrl').click();
    const ctrl = page.locator('[data-subapp="ctrl"]');
    await ctrl.getByTestId('ctrl-task-input').fill(text);
    await ctrl.getByTestId('ctrl-task-add').click();
    await ctrl.getByTestId('ctrl-task').getByTestId('ctrl-task-run').click();
    const run = ctrl.getByTestId('ctrl-run').first();
    await expect(run.getByTestId('ctrl-run-status')).toHaveText('done');
    await expect(ctrl.getByTestId('ctrl-decisions').first()).toContainText(
      move,
    );
    await expect(ctrl.getByTestId('ctrl-focus-setup')).toHaveCount(0);
    await expect(ctrl.getByTestId('ctrl-confirm')).toHaveCount(0);
    await expect(page.getByRole('dialog')).toHaveCount(0);
    expect(errors).toEqual([]);
  });
}

// ADR 0166: "open my downloads" shows that folder in a finder window without a card; nothing comes forward.
test('ctrl: open my downloads opens the folder in the finder without a card', async ({
  page,
}) => {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(e.message));
  await page.goto('/?dev=1');
  await page.getByTestId('nav-ctrl').click();
  const ctrl = page.locator('[data-subapp="ctrl"]');
  await ctrl.getByTestId('ctrl-task-input').fill('open my downloads');
  await ctrl.getByTestId('ctrl-task-add').click();
  await ctrl.getByTestId('ctrl-task').getByTestId('ctrl-task-run').click();
  const run = ctrl.getByTestId('ctrl-run').first();
  await expect(run.getByTestId('ctrl-run-status')).toHaveText('done');
  await expect(ctrl.getByTestId('ctrl-confirm')).toHaveCount(0);
  const decisions = ctrl.getByTestId('ctrl-decisions').first();
  await expect(decisions).toContainText('open a folder');
  await expect(decisions).toContainText(
    'show it in a finder window, nothing comes forward',
  );
  await expect(decisions).not.toContainText('clean up a folder');
  await expect(page.getByRole('dialog')).toHaveCount(0);
  expect(await axe(page, 'ctrl')).toEqual([]);
  expect(errors).toEqual([]);
});

// Run 40 (ADR 0166 amendment 1): the media folders open the same way; "my music folder" is the folder, not the app.
test('ctrl: open my pictures and my music folder open in the finder without a card', async ({
  page,
}) => {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(e.message));
  await page.goto('/?dev=1');
  await page.getByTestId('nav-ctrl').click();
  const ctrl = page.locator('[data-subapp="ctrl"]');
  for (const [text, folder] of [
    ['open my pictures', 'pictures'],
    ['open my music folder', 'music'],
  ]) {
    await ctrl.getByTestId('ctrl-task-input').fill(text);
    await ctrl.getByTestId('ctrl-task-add').click();
    await ctrl
      .getByTestId('ctrl-task')
      .filter({ hasText: text })
      .getByTestId('ctrl-task-run')
      .click();
    const run = ctrl.getByTestId('ctrl-run').first();
    await expect(run.getByTestId('ctrl-run-status')).toHaveText('done');
    await expect(ctrl.getByTestId('ctrl-confirm')).toHaveCount(0);
    const decisions = ctrl.getByTestId('ctrl-decisions').first();
    await expect(decisions).toContainText('open a folder');
    await expect(decisions).toContainText(folder);
    await expect(decisions).toContainText(
      'show it in a finder window, nothing comes forward',
    );
    await expect(decisions).not.toContainText('open an app');
  }
  await expect(page.getByRole('dialog')).toHaveCount(0);
  expect(await axe(page, 'ctrl')).toEqual([]);
  expect(errors).toEqual([]);
});

// ADR 0168: "every hour clear my clipboard" empties the clipboard without a card; nothing is read from it.
test('ctrl: clear my clipboard empties it without a card', async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(e.message));
  await page.goto('/?dev=1');
  await page.getByTestId('nav-ctrl').click();
  const ctrl = page.locator('[data-subapp="ctrl"]');
  await ctrl.getByTestId('ctrl-task-input').fill('clear my clipboard');
  await ctrl.getByTestId('ctrl-task-add').click();
  await ctrl.getByTestId('ctrl-task').getByTestId('ctrl-task-run').click();
  const run = ctrl.getByTestId('ctrl-run').first();
  await expect(run.getByTestId('ctrl-run-status')).toHaveText('done');
  await expect(ctrl.getByTestId('ctrl-confirm')).toHaveCount(0);
  const decisions = ctrl.getByTestId('ctrl-decisions').first();
  await expect(decisions).toContainText('clear your clipboard');
  await expect(decisions).toContainText(
    'empty the clipboard, nothing read from it',
  );
  await expect(decisions).not.toContainText('clean up a folder');
  await expect(page.getByRole('dialog')).toHaveCount(0);
  expect(await axe(page, 'ctrl')).toEqual([]);
  expect(errors).toEqual([]);
});

// ADR 0173: "hide the dock" / "always show the dock" set the dock's auto-hide through system events, no card.
test('ctrl: hide the dock and always show the dock set its auto-hide without a card', async ({
  page,
}) => {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(e.message));
  await page.goto('/?dev=1');
  await page.getByTestId('nav-ctrl').click();
  const ctrl = page.locator('[data-subapp="ctrl"]');
  for (const [text, move] of [
    ['hide the dock', 'hide the dock until the pointer reaches it'],
    ['always show the dock', 'always show the dock'],
  ] as const) {
    await ctrl.getByTestId('ctrl-task-input').fill(text);
    await ctrl.getByTestId('ctrl-task-add').click();
    const task = ctrl.getByTestId('ctrl-task').filter({ hasText: text });
    await task.getByTestId('ctrl-task-run').click();
    const run = ctrl.getByTestId('ctrl-run').first();
    await expect(run.getByTestId('ctrl-run-status')).toHaveText('done');
    await expect(ctrl.getByTestId('ctrl-confirm')).toHaveCount(0);
    const decisions = ctrl.getByTestId('ctrl-decisions').first();
    await expect(decisions).toContainText('hide or show the dock');
    await expect(decisions).toContainText(move);
    await expect(decisions).not.toContainText('hide an app');
    await expect(decisions).not.toContainText('show a hidden app');
  }
  await expect(page.getByRole('dialog')).toHaveCount(0);
  expect(await axe(page, 'ctrl')).toEqual([]);
  expect(errors).toEqual([]);
});

// ADR 0176: "eject my disks" / "eject the usb drive" unmount the ejectable disks in the helper, never forced, no card.
test('ctrl: eject my disks and eject the usb drive eject without a card', async ({
  page,
}) => {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(e.message));
  await page.goto('/?dev=1');
  await page.getByTestId('nav-ctrl').click();
  const ctrl = page.locator('[data-subapp="ctrl"]');
  for (const [text, move] of [
    [
      'every day at 6pm eject all my drives',
      'eject every external disk, none forced',
    ],
    ['eject the usb drive', 'eject the one external disk, never forced'],
  ] as const) {
    await ctrl.getByTestId('ctrl-task-input').fill(text);
    await ctrl.getByTestId('ctrl-task-add').click();
    const task = ctrl.getByTestId('ctrl-task').filter({ hasText: text });
    await task.getByTestId('ctrl-task-run').click();
    const run = ctrl.getByTestId('ctrl-run').first();
    await expect(run.getByTestId('ctrl-run-status')).toHaveText('done');
    await expect(ctrl.getByTestId('ctrl-confirm')).toHaveCount(0);
    const decisions = ctrl.getByTestId('ctrl-decisions').first();
    await expect(decisions).toContainText('eject your disks');
    await expect(decisions).toContainText(move);
  }
  await expect(page.getByRole('dialog')).toHaveCount(0);
  expect(await axe(page, 'ctrl')).toEqual([]);
  expect(errors).toEqual([]);
});

// ADR 0135: "every night at 10 mute my mac" mutes the sound output without a card (reversible), no app touched.
test('ctrl: every night mute my mac mutes the sound without a card', async ({
  page,
}) => {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(e.message));
  await page.goto('/?dev=1');
  await page.getByTestId('nav-ctrl').click();
  const ctrl = page.locator('[data-subapp="ctrl"]');
  await ctrl
    .getByTestId('ctrl-task-input')
    .fill('every night at 10 mute my mac');
  await ctrl.getByTestId('ctrl-schedule-daily').click();
  await ctrl.getByTestId('ctrl-schedule-time').fill('22:00');
  await ctrl.getByTestId('ctrl-task-add').click();
  await ctrl.getByTestId('ctrl-task').getByTestId('ctrl-task-run').click();
  const run = ctrl.getByTestId('ctrl-run').first();
  await expect(run.getByTestId('ctrl-run-status')).toHaveText('done');
  await expect(ctrl.getByTestId('ctrl-confirm')).toHaveCount(0);
  const decisions = ctrl.getByTestId('ctrl-decisions').first();
  await expect(decisions).toContainText('mute your mac');
  await expect(decisions).toContainText('mute the sound output');
  await expect(page.getByRole('dialog')).toHaveCount(0);
  expect(await axe(page, 'ctrl')).toEqual([]);
  expect(errors).toEqual([]);
});

test('ctrl: every night turn on dark mode switches the appearance with no card (ADR 0137)', async ({
  page,
}) => {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(e.message));
  await page.goto('/?dev=1');
  await page.getByTestId('nav-ctrl').click();
  const ctrl = page.locator('[data-subapp="ctrl"]');
  await ctrl
    .getByTestId('ctrl-task-input')
    .fill('every night turn on dark mode');
  await ctrl.getByTestId('ctrl-schedule-daily').click();
  await ctrl.getByTestId('ctrl-schedule-time').fill('22:00');
  await ctrl.getByTestId('ctrl-task-add').click();
  await ctrl.getByTestId('ctrl-task').getByTestId('ctrl-task-run').click();
  const run = ctrl.getByTestId('ctrl-run').first();
  await expect(run.getByTestId('ctrl-run-status')).toHaveText('done');
  await expect(ctrl.getByTestId('ctrl-confirm')).toHaveCount(0);
  const decisions = ctrl.getByTestId('ctrl-decisions').first();
  await expect(decisions).toContainText('switch dark mode');
  await expect(decisions).toContainText('switch to dark mode');
  await expect(page.getByRole('dialog')).toHaveCount(0);
  expect(await axe(page, 'ctrl')).toEqual([]);
  expect(errors).toEqual([]);
});

test('ctrl: every evening toggle dark mode flips the appearance with no card (ADR 0145)', async ({
  page,
}) => {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(e.message));
  await page.goto('/?dev=1');
  await page.getByTestId('nav-ctrl').click();
  const ctrl = page.locator('[data-subapp="ctrl"]');
  await ctrl
    .getByTestId('ctrl-task-input')
    .fill('every evening toggle dark mode');
  await ctrl.getByTestId('ctrl-schedule-daily').click();
  await ctrl.getByTestId('ctrl-schedule-time').fill('19:00');
  await ctrl.getByTestId('ctrl-task-add').click();
  await ctrl.getByTestId('ctrl-task').getByTestId('ctrl-task-run').click();
  const run = ctrl.getByTestId('ctrl-run').first();
  await expect(run.getByTestId('ctrl-run-status')).toHaveText('done');
  await expect(ctrl.getByTestId('ctrl-confirm')).toHaveCount(0);
  const decisions = ctrl.getByTestId('ctrl-decisions').first();
  await expect(decisions).toContainText('switch dark mode');
  await expect(decisions).toContainText('flip the appearance');
  await expect(page.getByRole('dialog')).toHaveCount(0);
  expect(await axe(page, 'ctrl')).toEqual([]);
  expect(errors).toEqual([]);
});

test('ctrl: every morning open my email finds mail already open, nothing brought forward (ADR 0138)', async ({
  page,
}) => {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(e.message));
  await page.goto('/?dev=1');
  await page.getByTestId('nav-ctrl').click();
  const ctrl = page.locator('[data-subapp="ctrl"]');
  await ctrl.getByTestId('ctrl-task-input').fill('every morning open my email');
  await ctrl.getByTestId('ctrl-schedule-daily').click();
  await ctrl.getByTestId('ctrl-schedule-time').fill('22:00');
  await ctrl.getByTestId('ctrl-task-add').click();
  await ctrl.getByTestId('ctrl-task').getByTestId('ctrl-task-run').click();
  const run = ctrl.getByTestId('ctrl-run').first();
  await expect(run.getByTestId('ctrl-run-status')).toHaveText('done');
  await expect(ctrl.getByTestId('ctrl-confirm')).toHaveCount(0);
  const decisions = ctrl.getByTestId('ctrl-decisions').first();
  await expect(decisions).toContainText('open an app');
  await expect(decisions).toContainText('mail');
  await expect(decisions).toContainText('start it hidden');
  await expect(page.getByRole('dialog')).toHaveCount(0);
  expect(await axe(page, 'ctrl')).toEqual([]);
  expect(errors).toEqual([]);
});

test('ctrl: every morning unmute my mac turns the sound back on without a card (ADR 0139)', async ({
  page,
}) => {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(e.message));
  await page.goto('/?dev=1');
  await page.getByTestId('nav-ctrl').click();
  const ctrl = page.locator('[data-subapp="ctrl"]');
  await ctrl.getByTestId('ctrl-task-input').fill('every morning unmute my mac');
  await ctrl.getByTestId('ctrl-schedule-daily').click();
  await ctrl.getByTestId('ctrl-schedule-time').fill('22:00');
  await ctrl.getByTestId('ctrl-task-add').click();
  await ctrl.getByTestId('ctrl-task').getByTestId('ctrl-task-run').click();
  const run = ctrl.getByTestId('ctrl-run').first();
  await expect(run.getByTestId('ctrl-run-status')).toHaveText('done');
  await expect(ctrl.getByTestId('ctrl-confirm')).toHaveCount(0);
  const decisions = ctrl.getByTestId('ctrl-decisions').first();
  await expect(decisions).toContainText('unmute your mac');
  await expect(decisions).toContainText('turn the sound output back on');
  await expect(page.getByRole('dialog')).toHaveCount(0);
  expect(await axe(page, 'ctrl')).toEqual([]);
  expect(errors).toEqual([]);
});

test('ctrl: every evening set the volume to 30 sets that level without a card (ADR 0140)', async ({
  page,
}) => {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(e.message));
  await page.goto('/?dev=1');
  await page.getByTestId('nav-ctrl').click();
  const ctrl = page.locator('[data-subapp="ctrl"]');
  await ctrl
    .getByTestId('ctrl-task-input')
    .fill('every evening set the volume to 30');
  await ctrl.getByTestId('ctrl-schedule-daily').click();
  await ctrl.getByTestId('ctrl-schedule-time').fill('19:00');
  await ctrl.getByTestId('ctrl-task-add').click();
  await ctrl.getByTestId('ctrl-task').getByTestId('ctrl-task-run').click();
  const run = ctrl.getByTestId('ctrl-run').first();
  await expect(run.getByTestId('ctrl-run-status')).toHaveText('done');
  await expect(ctrl.getByTestId('ctrl-confirm')).toHaveCount(0);
  const decisions = ctrl.getByTestId('ctrl-decisions').first();
  await expect(decisions).toContainText('set the volume');
  await expect(decisions).toContainText('30%');
  await expect(decisions).toContainText('set the sound output to that level');
  await expect(page.getByRole('dialog')).toHaveCount(0);
  expect(await axe(page, 'ctrl')).toEqual([]);
  expect(errors).toEqual([]);
});

test('ctrl: turn it up a bit steps the volume from where it is without a card (ADR 0143)', async ({
  page,
}) => {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(e.message));
  await page.goto('/?dev=1');
  await page.getByTestId('nav-ctrl').click();
  const ctrl = page.locator('[data-subapp="ctrl"]');
  for (const [text, step, move] of [
    ['turn it up a bit', 'by 5', 'turn the sound output up a step'],
    [
      'turn the volume down by 20',
      'by 20',
      'turn the sound output down a step',
    ],
  ] as const) {
    await ctrl.getByTestId('ctrl-task-input').fill(text);
    await ctrl.getByTestId('ctrl-task-add').click();
    const task = ctrl.getByTestId('ctrl-task').filter({ hasText: text });
    await task.getByTestId('ctrl-task-run').click();
    const run = ctrl.getByTestId('ctrl-run').first();
    await expect(run.getByTestId('ctrl-run-status')).toHaveText('done');
    await expect(ctrl.getByTestId('ctrl-confirm')).toHaveCount(0);
    const decisions = ctrl.getByTestId('ctrl-decisions').first();
    await expect(decisions).toContainText('turn the volume up or down');
    await expect(decisions).toContainText(step);
    await expect(decisions).toContainText(move);
  }
  await expect(page.getByRole('dialog')).toHaveCount(0);
  expect(await axe(page, 'ctrl')).toEqual([]);
  expect(errors).toEqual([]);
});

test('ctrl: switched off in the module manager, ctrl has no tab and runs nothing; back on, its tasks are there (ADR 0146)', async ({
  page,
}) => {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(e.message));
  await page.goto('/?dev=1');
  await page.getByTestId('nav-ctrl').click();
  const ctrl = page.locator('[data-subapp="ctrl"]');
  await ctrl.getByTestId('ctrl-task-input').fill('every day close all my tabs');
  await ctrl.getByTestId('ctrl-schedule-daily').click();
  await ctrl.getByTestId('ctrl-schedule-time').fill('09:00');
  await ctrl.getByTestId('ctrl-task-add').click();
  await expect(ctrl.getByTestId('ctrl-task')).toHaveCount(1);

  // ADR 0161: the switch is in marketplace → installed.
  await page.getByTestId('nav-marketplace').click();
  const market = page.locator('[data-subapp="marketplace"]');
  await market.getByTestId('tab-installed').click();
  const sw = market.getByTestId('module-enabled-ctrl');
  await sw.click();
  await expect(page.getByTestId('nav-ctrl')).toHaveCount(0);
  await expect(page.locator('[data-subapp="ctrl"]')).toHaveCount(0);
  // Like the real gate, the mock refuses ctrl:* while ctrl is off: nothing of ctrl's is called or runs.
  const ctrlCalls = () =>
    page.evaluate(
      () =>
        (
          window as unknown as {
            __qrtzMock: { calls: { channel: string }[] };
          }
        ).__qrtzMock.calls.filter((c) => c.channel.startsWith('ctrl:')).length,
    );
  const before = await ctrlCalls();
  await page.waitForTimeout(300);
  expect(await ctrlCalls()).toBe(before);
  expect(await axe(page, 'marketplace')).toEqual([]);

  await sw.click();
  await page.getByTestId('nav-ctrl').click();
  const task = page.locator('[data-subapp="ctrl"]').getByTestId('ctrl-task');
  await expect(task).toHaveCount(1);
  await expect(task.getByTestId('ctrl-task-schedule')).toHaveText(
    'every day at 09:00 · never run',
  );
  expect(errors).toEqual([]);
});
