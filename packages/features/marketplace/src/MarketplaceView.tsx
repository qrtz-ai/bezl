import {
  useCallback,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from 'react';
import {
  AppList,
  AppRow,
  Button,
  Callout,
  Dialog,
  EmptyState,
  ExtensionHeader,
  FactList,
  IconButton,
  IconLine,
  IconLines,
  InstallProgress,
  Menu,
  Modal,
  Page,
  RowProgress,
  SettingRow,
  SettingsSection,
  Spinner,
  Stack,
  Switch,
  Text,
  Toolbar,
  formatBytes,
  useToast,
  type ExtensionStatus,
  type MenuItem,
  ErrorCallout,
  userError,
  type UserError,
} from '@qrtz/ui';
import {
  MODULE_IDS,
  isModuleInstalled,
  isModuleOn,
  isBundledModule,
  loadConfig,
  LOG_MESSAGE_MAX,
  requestNavigate,
  type CatalogEntry,
  type Config,
  type OsPermission,
  type PackageOp,
  type PackagePlan,
  type PackageRow,
  type PackagesProgress,
  type PackagesState,
  type SettingsPane,
} from '@qrtz/core';
import { useQrtz, useSubAppRoute } from '@qrtz/core/react';
import {
  KIND_ICON,
  MODEL_APPS,
  PERMISSIONS,
  infoFor,
  installedRow,
  isModelApp,
  isModuleId,
  listings,
  lockedDependencies,
  marketPath,
  parseMarketRoute,
  metaLine,
  statusOf,
  type MarketRoute,
} from './catalog';
import { ModulePermissions, usePermissions } from './ModulePermissions';
import { AppModels } from './AppModels';

type Verb = 'install' | 'update' | 'remove';
/** The host's answer while modules.qrtz.ai isn't live (packages_ipc NOT_SET_UP: details.reason "not-set-up"). */
export const isNotSetUp = (e: unknown) =>
  (e as { options?: { details?: { reason?: string } } } | null)?.options
    ?.details?.reason === 'not-set-up';
/**
 * The store couldn't reach modules.qrtz.ai, or the host answered 404 (an empty bucket): packages_ipc maps
 * InstallError::Network ("every host failed: …") to a retryable E_UNAVAILABLE. Shown as a friendly
 * "try again" with a retry, never the raw text (which goes to the local log instead).
 */
export const isStoreUnavailable = (e: unknown) => {
  const x = e as { code?: string; message?: string } | null;
  return (
    x?.code === 'E_UNAVAILABLE' &&
    !isNotSetUp(e) &&
    /^every host failed\b/i.test(x.message ?? '')
  );
};
/** The friendly line for isStoreUnavailable. */
export const STORE_UNAVAILABLE =
  "the store isn't available right now, try again in a bit";
/**
 * The shared section's "used by" line, from the host's `usedBy` (qrtz.lock's extensions plus the modules a pinned
 * model is for): the same answer `packages:prune` keeps by, so a row never reads "used by nothing" while clean up
 * keeps it.
 */
export const usedByLine = (r: PackageRow) =>
  r.usedBy.length
    ? `used by ${r.usedBy.join(', ')}`
    : r.requested
      ? 'you installed it'
      : 'used by nothing';
/** Clean up is offered only when the host's own dry run of prune would delete something. */
export const canPrune = (s: PackagesState | null) =>
  !!s && s.reclaimable.dirs + s.reclaimable.blobs > 0;
const PAST: Record<Verb, string> = {
  install: 'installed',
  update: 'updated',
  remove: 'removed',
};
const ING: Record<Verb, string> = {
  install: 'installing',
  update: 'updating',
  remove: 'removing',
};

/**
 * The qrtz page (ADR 0161, Jared 2026-10-18): the app manager, under the shared page header ("qrtz", and a gear
 * that opens qrtz's own settings at /qrtz/settings). An "installed" list, then an "available" one, each app a
 * short row; clicking a row opens its detail modal (`item/NAME`): description, version, size with what it pulls
 * in, permissions, models, versions, and install / update / remove / open. Opening it never goes online: "check
 * for apps" and an install the user confirms are the only network, and an install shows its plan (what is added,
 * removed and downloaded) before anything moves.
 */
export default function MarketplaceView() {
  const qrtz = useQrtz();
  const toast = useToast();
  const route = useSubAppRoute();
  const at = parseMarketRoute(route.path);
  const go = useCallback(
    (r: MarketRoute) => route.navigate(marketPath(r)),
    [route],
  );
  const [state, setState] = useState<PackagesState | null>(null);
  const [cfg, setCfg] = useState<Pick<Config, 'modules'> | null>(null);
  const [error, setError] = useState<UserError | null>(null);
  const [checking, setChecking] = useState(false);
  const [pending, setPending] = useState<{
    name: string;
    verb: Verb;
    plan: PackagePlan;
  } | null>(null);
  const [working, setWorking] = useState<{ name: string; verb: Verb } | null>(
    null,
  );
  const [progress, setProgress] = useState<PackagesProgress | null>(null);
  /** The apps whose last install or update failed: their row shows the error and a retry. */
  const [failed, setFailed] = useState<Readonly<Record<string, Verb>>>({});

  // modules.qrtz.ai isn't live yet (no root pinned in this build): a friendly "coming soon", not an error.
  const [soon, setSoon] = useState(false);
  // Bundled echo has no index entry: its row shows the app's version ("v0.2.0 · built in").
  const [appVersion, setAppVersion] = useState<string | null>(null);
  const [unavailable, setUnavailable] = useState(false);
  const fail = useCallback(
    (e: Error) => {
      if (isNotSetUp(e)) setSoon(true);
      else if (isStoreUnavailable(e)) {
        setUnavailable(true);
        // The raw reason (hosts and paths only, no user content) goes to qrtz.log for diagnosis.
        void qrtz
          .invoke('log:write', {
            level: 'warn',
            scope: 'marketplace',
            message: `store unavailable: ${e.message}`.slice(
              0,
              LOG_MESSAGE_MAX,
            ),
          })
          .catch(() => {});
      } else setError(userError(e, 'finish that in the store'));
    },
    [qrtz],
  );
  const load = useCallback(
    () => qrtz.invoke('packages:list').then(setState, fail),
    [qrtz, fail],
  );
  useEffect(() => {
    void load();
    const apply = (c: Record<string, unknown>) => setCfg(loadConfig(c));
    qrtz.invoke('config:get').then(apply, () => {});
    qrtz.invoke('app:get-info').then(
      (i) => setAppVersion(i.version),
      () => {},
    );
    const offs = [
      qrtz.on('config:changed', apply),
      qrtz.on('packages:progress', setProgress),
    ];
    return () => offs.forEach((o) => o());
  }, [qrtz, load]);

  const rows = useMemo(() => state?.packages ?? [], [state]);
  const available = useMemo(() => state?.available ?? [], [state]);
  const entryOf = (name: string) => available.find((e) => e.name === name);
  const statusFor = (name: string): ExtensionStatus =>
    working?.name === name
      ? 'working'
      : statusOf(name, entryOf(name), rows, hostInstalled(name));

  const check = () => {
    setError(null);
    setUnavailable(false);
    setChecking(true);
    qrtz
      .invoke('packages:refresh')
      .then(setState, fail)
      .finally(() => setChecking(false));
  };
  const markFailed = (name: string, verb: Verb) =>
    verb !== 'remove' && setFailed((f) => ({ ...f, [name]: verb }));
  const propose = (name: string, verb: Verb) => {
    setError(null);
    setUnavailable(false);
    setFailed((f) => {
      const { [name]: _gone, ...rest } = f;
      return rest;
    });
    const op: PackageOp =
      verb === 'install'
        ? { op: 'install', names: [name] }
        : verb === 'update'
          ? { op: 'update', names: [name], latest: true }
          : { op: 'remove', names: [name] };
    qrtz.invoke('packages:plan', op).then(
      (plan) => setPending({ name, verb, plan }),
      (e: Error) => {
        markFailed(name, verb);
        fail(e);
      },
    );
  };
  const confirm = () => {
    const p = pending;
    if (!p) return;
    setPending(null);
    setProgress(null);
    setWorking({ name: p.name, verb: p.verb });
    qrtz
      .invoke('packages:apply', {
        planSha256: p.plan.planSha256,
        downloadBytes: p.plan.downloadBytes,
      })
      .then(
        () => {
          toast(`${p.name} ${PAST[p.verb]}`, 'success');
          return load();
        },
        (e: Error) => {
          markFailed(p.name, p.verb);
          fail(e);
        },
      )
      .finally(() => {
        setWorking(null);
        setProgress(null);
      });
  };
  const prune = () =>
    qrtz.invoke('packages:prune').then((r) => {
      toast(
        r.dirs || r.blobs
          ? `freed ${formatBytes(r.bytes)}`
          : 'nothing to clean up',
        'success',
      );
      return load();
    }, fail);
  const { perms, ask, open, restart, pending: asking } = usePermissions();
  const allow = (p: OsPermission) => void ask(p).catch(fail);
  const openPane = (p: SettingsPane) => void open(p).catch(fail);
  const relaunch = () => void restart().catch(fail);
  // ADR 0169: a module's models come with its install (the plan lists them, the one consent covers them, and
  // they go when nothing installed needs them), so switching a module on downloads nothing.
  const setEnabled = (name: string, on: boolean) => {
    if (!isModuleId(name)) return;
    qrtz
      .invoke('packages:set-enabled', { name, enabled: on })
      .then(() => qrtz.invoke('config:get'))
      .then((c) => setCfg(loadConfig(c)), fail);
  };
  const enabled = (name: string) =>
    isModuleId(name) && cfg ? isModuleOn(cfg, name) : true;
  /** The host's `modules.<id>.installed` (from qrtz.lock; the mock and a dev monolith count every module). */
  const hostInstalled = (name: string) =>
    isModuleId(name) && !!cfg && isModuleInstalled(cfg, name);
  const installedIds = MODULE_IDS.filter((id) => hostInstalled(id));

  const pct =
    progress && progress.totalBytes
      ? progress.doneBytes / progress.totalBytes
      : 0;
  const busy = working !== null || pending !== null;
  // ADR 0172: an update reaches a running module at the next launch; until then it runs its old version.
  const updated = state?.pendingRestart ?? [];
  const restartPending = (name: string) =>
    (updated as readonly string[]).includes(name);
  const openApp = (name: string) => {
    const view = infoFor(name).view;
    if (view) requestNavigate({ app: view });
  };
  const appSettings = (name: string) => {
    const view = infoFor(name).view;
    if (view) requestNavigate({ app: view, settings: true });
  };
  /** Echo ships inside qrtz.app (ADR 0174): never removed, from the row or the modal. */
  const removable = (name: string) =>
    !!installedRow(name, rows) && !isBundledModule(name);

  /** The modal's main button: install / update / open. */
  const primary = (name: string) => {
    const s = statusFor(name);
    const view = infoFor(name).view;
    if (s === 'working')
      return (
        <Button variant="primary" loading disabled>
          {ING[working?.verb ?? 'install']}
        </Button>
      );
    if (s === 'available')
      return (
        <Button
          variant="primary"
          icon="download"
          disabled={busy}
          onClick={() => propose(name, 'install')}
          data-testid={`extension-install-${name}`}
        >
          {failed[name] ? 'retry' : 'install'}
        </Button>
      );
    if (s === 'update')
      return (
        <Button
          variant="primary"
          icon="update"
          disabled={busy}
          onClick={() => propose(name, 'update')}
          data-testid={`extension-update-${name}`}
        >
          update
        </Button>
      );
    return view ? (
      <Button
        variant="secondary"
        disabled={!enabled(name)}
        onClick={() => openApp(name)}
        data-testid={`extension-launch-${name}`}
      >
        open
      </Button>
    ) : null;
  };

  /** The row's action on the right: a download icon, the inline progress, or the "more actions" menu. */
  const rowAction = (name: string, installed: boolean) => {
    const s = statusFor(name);
    if (s === 'working')
      return working?.verb === 'remove' ? (
        <Spinner label={`removing ${name}`} />
      ) : (
        <RowProgress
          value={pct}
          label={`${ING[working?.verb ?? 'install']} ${name}`}
          data-testid={`app-progress-${name}`}
        />
      );
    if (!installed)
      return failed[name] ? (
        <IconButton
          icon="refresh"
          tone="danger"
          label={`retry downloading ${name}`}
          disabled={busy}
          onClick={() => propose(name, 'install')}
          data-testid={`app-retry-${name}`}
        />
      ) : (
        <IconButton
          icon="download"
          label={`download ${name}`}
          disabled={busy}
          onClick={() => propose(name, 'install')}
          data-testid={`app-download-${name}`}
        />
      );
    const view = infoFor(name).view;
    const on = enabled(name);
    const items: MenuItem[] = [
      ...(view
        ? [
            {
              id: 'open',
              label: 'open',
              icon: 'open' as const,
              disabled: !on,
              onSelect: () => openApp(name),
            },
          ]
        : []),
      ...(s === 'update'
        ? [
            {
              id: 'update',
              label: failed[name] ? 'retry update' : 'update',
              icon: 'update' as const,
              disabled: busy,
              onSelect: () => propose(name, 'update'),
            },
          ]
        : []),
      ...(restartPending(name)
        ? [
            {
              id: 'restart',
              label: 'restart to update',
              icon: 'restart' as const,
              onSelect: relaunch,
            },
          ]
        : []),
      ...(view
        ? [
            {
              id: 'settings',
              label: 'app opts',
              icon: 'settings' as const,
              disabled: !on,
              onSelect: () => appSettings(name),
            },
          ]
        : []),
      ...(removable(name)
        ? [
            {
              id: 'remove',
              label: 'remove',
              icon: 'trash' as const,
              danger: true,
              disabled: busy,
              onSelect: () => propose(name, 'remove'),
            },
          ]
        : []),
    ];
    return (
      <Menu
        label={`more actions for ${name}`}
        items={items}
        badge={s === 'update'}
        data-testid={`app-menu-${name}`}
      />
    );
  };

  /** The short word beside a row's action. */
  const rowStatus = (
    name: string,
    installed: boolean,
  ): { text: string; tone: 'muted' | 'warning' | 'danger' } | null => {
    const s = statusFor(name);
    if (s === 'working') return null;
    if (failed[name])
      return {
        text:
          failed[name] === 'update' ? "couldn't update" : "couldn't download",
        tone: 'danger',
      };
    if (!installed) return null;
    if (restartPending(name))
      return { text: 'restart to update', tone: 'warning' };
    if (s === 'update') return { text: 'update', tone: 'warning' };
    if (!enabled(name)) return { text: 'off', tone: 'muted' };
    if (isBundledModule(name)) return { text: 'built in', tone: 'muted' };
    return null;
  };

  const consent = pending ? (
    <Dialog
      open
      onClose={() => setPending(null)}
      title={`${pending.verb} ${pending.name}`}
    >
      <Stack gap={4} pad={5} data-testid="install-consent">
        {pending.plan.changes.length ? (
          <IconLines aria-label="changes" data-testid="consent-changes">
            {pending.plan.changes.map((c) => (
              <IconLine
                key={`${c.change}-${c.name}`}
                icon={c.change === 'add' ? 'download' : 'trash'}
                title={`${c.change === 'add' ? 'add' : 'remove'} ${c.name}`}
                meta={`v${c.version}`}
              />
            ))}
          </IconLines>
        ) : (
          <Text muted>nothing changes.</Text>
        )}
        <Text variant="caption" muted data-testid="consent-download">
          {pending.plan.downloadBytes
            ? `downloads ${formatBytes(pending.plan.downloadBytes)} of signed packages from modules.qrtz.ai (models can come from huggingface.co). nothing else goes online, and every file is checked before it is used.`
            : 'nothing to download. this stays on this mac.'}
        </Text>
        <Stack dir="row" gap={2} justify="end">
          <Button variant="ghost" onClick={() => setPending(null)}>
            cancel
          </Button>
          <Button
            variant={pending.verb === 'remove' ? 'danger' : 'primary'}
            onClick={confirm}
            data-testid="consent-confirm"
          >
            {pending.plan.downloadBytes
              ? `${pending.verb} · ${formatBytes(pending.plan.downloadBytes)}`
              : pending.verb}
          </Button>
        </Stack>
      </Stack>
    </Dialog>
  ) : null;

  const soonBox = soon ? (
    <Callout
      tone="info"
      title="apps coming soon"
      data-testid="marketplace-soon"
    >
      apps will appear here once qrtz.ai&apos;s app server is live. qrtz starts
      empty until then.
    </Callout>
  ) : null;

  const unavailableBox = unavailable ? (
    <Callout
      tone="warning"
      title={STORE_UNAVAILABLE}
      data-testid="marketplace-unavailable"
      action={
        <Button
          variant="secondary"
          size="sm"
          icon="refresh"
          loading={checking}
          disabled={checking}
          onClick={check}
          data-testid="marketplace-retry"
        >
          retry
        </Button>
      }
    />
  ) : null;

  const errorBox = (
    <ErrorCallout
      error={error}
      onDismiss={() => setError(null)}
      data-testid="marketplace-error"
    />
  );

  const restartBox = updated.length ? (
    <Callout
      tone="info"
      title="restart to finish updating"
      data-testid="marketplace-restart"
      action={
        <Button
          variant="secondary"
          size="sm"
          icon="restart"
          onClick={relaunch}
          data-testid="marketplace-restart-button"
        >
          restart qrtz
        </Button>
      }
    >
      {`${updated.join(', ')} ${updated.length === 1 ? 'keeps' : 'keep'} running the old version until qrtz restarts.`}
    </Callout>
  ) : null;

  const installBar = (name: string) =>
    working && working.name === name && working.verb !== 'remove' ? (
      <InstallProgress
        value={pct}
        label={`${ING[working.verb]} ${working.name}`}
        detail={
          progress
            ? `${ING[working.verb]} ${working.name}: ${formatBytes(progress.doneBytes)} of ${formatBytes(progress.totalBytes)}`
            : `${ING[working.verb]} ${working.name}…`
        }
      />
    ) : null;

  const installedNames = [
    ...new Set([
      ...rows.filter((r) => r.kind === 'extension').map((r) => r.name),
      ...installedIds.filter((n) => !installedRow(n, rows)),
    ]),
  ];
  const availableNames = listings(available, installedIds)
    .map((l) => l.name)
    .filter((n) => !installedNames.includes(n));
  const shared = rows.filter((r) => r.kind !== 'extension');
  const onIds = MODEL_APPS.filter((id) => hostInstalled(id) && enabled(id));

  const row = (name: string, installed: boolean) => {
    const info = infoFor(name);
    const r = installedRow(name, rows);
    const deps = r ? lockedDependencies(r, rows) : [];
    const st = rowStatus(name, installed);
    return (
      <AppRow
        key={name}
        name={name}
        icon={info.icon}
        description={info.description}
        meta={
          r
            ? `v${r.version} · ${formatBytes(r.sizeBytes + deps.reduce((n, d) => n + d.sizeBytes, 0))}`
            : metaLine(name, entryOf(name), appVersion)
        }
        status={st?.text ?? null}
        statusTone={st?.tone ?? 'muted'}
        action={rowAction(name, installed)}
        onOpen={() => go({ page: 'item', name })}
      />
    );
  };

  const item = at.page === 'item' ? at.name : null;
  const modal = item ? (
    <Modal
      open
      title={item}
      closeLabel={`close ${item}`}
      onClose={() => go({ page: 'list' })}
      data-testid="app-modal"
    >
      <Stack gap={6}>
        {installBar(item)}
        <Detail
          name={item}
          entry={entryOf(item)}
          status={statusFor(item)}
          action={primary(item)}
          rows={rows}
          appVersion={appVersion}
        />
        {isModuleId(item) && hostInstalled(item) ? (
          <SettingsSection
            variant="plain"
            title="on this mac"
            data-testid="extension-switch"
          >
            <SettingRow label={`${item} on`}>
              <Switch
                label={`${item} on`}
                checked={enabled(item)}
                onChange={(v) => setEnabled(item, v)}
                disabled={!!installedRow(item, rows)?.disabled}
                data-testid={`module-enabled-${item}`}
              />
            </SettingRow>
            {enabled(item) ? (
              <ModulePermissions
                module={item}
                perms={perms}
                onAsk={allow}
                onOpen={openPane}
                onRestart={relaunch}
                pending={asking}
              />
            ) : (
              <Text variant="caption" muted>
                {`${item} is off: no sidebar row, no hotkeys, nothing running.`}
              </Text>
            )}
          </SettingsSection>
        ) : null}
        {isModelApp(item) && hostInstalled(item) ? (
          <AppModels app={item} on={onIds} />
        ) : null}
        <Stack dir="row" gap={2} wrap>
          {restartPending(item) ? (
            <Button
              variant="secondary"
              icon="restart"
              onClick={relaunch}
              data-testid={`extension-restart-${item}`}
            >
              restart to update
            </Button>
          ) : null}
          {infoFor(item).view && hostInstalled(item) ? (
            <Button
              variant="ghost"
              icon="settings"
              disabled={!enabled(item)}
              onClick={() => appSettings(item)}
              data-testid={`module-settings-${item}`}
            >
              app opts
            </Button>
          ) : null}
          {removable(item) ? (
            <Button
              variant="danger"
              icon="trash"
              disabled={busy}
              onClick={() => propose(item, 'remove')}
              data-testid={`extension-remove-${item}`}
            >
              remove
            </Button>
          ) : null}
        </Stack>
      </Stack>
    </Modal>
  ) : null;

  return (
    <Page data-testid="qrtz-page" title="apps">
      <Toolbar
        end={
          <Button
            size="sm"
            variant="secondary"
            icon="refresh"
            loading={checking}
            disabled={checking}
            onClick={check}
            data-testid="marketplace-check"
          >
            check for apps
          </Button>
        }
      >
        <Text variant="caption" muted data-testid="marketplace-index">
          {state?.index
            ? `signed list v${state.index.version} · checked ${new Date(state.index.issuedMs).toLocaleDateString().toLowerCase()}`
            : 'checking goes online to modules.qrtz.ai for the signed list. opening this page never does.'}
        </Text>
      </Toolbar>
      {soonBox}
      {unavailableBox}
      {errorBox}
      {restartBox}
      <SettingsSection
        variant="plain"
        title="installed"
        data-testid="qrtz-installed"
      >
        {installedNames.length ? (
          <AppList aria-label="installed apps" data-testid="installed-list">
            {installedNames.map((n) => row(n, true))}
          </AppList>
        ) : (
          <Text variant="caption" muted data-testid="installed-empty">
            nothing installed yet. qrtz starts empty: download an app below.
          </Text>
        )}
      </SettingsSection>
      <SettingsSection
        variant="plain"
        title="available"
        data-testid="qrtz-available"
      >
        {availableNames.length ? (
          <AppList aria-label="available apps" data-testid="available-list">
            {availableNames.map((n) => row(n, false))}
          </AppList>
        ) : installedNames.length && available.length ? (
          <Text variant="caption" muted data-testid="available-none">
            every app is installed.
          </Text>
        ) : (
          <EmptyState
            icon="package"
            title={
              soon
                ? 'nothing to install yet.'
                : 'qrtz starts empty. check for apps to install your first one.'
            }
            action={
              soon ? null : (
                <Button
                  variant="primary"
                  size="sm"
                  icon="refresh"
                  loading={checking}
                  disabled={checking}
                  onClick={check}
                  data-testid="marketplace-empty-check"
                >
                  check for apps
                </Button>
              )
            }
          />
        )}
      </SettingsSection>
      {shared.length || canPrune(state) ? (
        <SettingsSection
          variant="plain"
          title="shared"
          data-testid="module-shared"
        >
          {shared.length ? (
            <IconLines aria-label="shared packages">
              {shared.map((d) => (
                <IconLine
                  key={`${d.name}@${d.version}`}
                  icon={KIND_ICON[d.kind]}
                  title={d.name}
                  meta={`${formatBytes(d.sizeBytes)} · ${usedByLine(d)}`}
                />
              ))}
            </IconLines>
          ) : null}
          {canPrune(state) ? (
            <Stack dir="row" gap={2} align="center">
              <Button
                size="sm"
                variant="secondary"
                icon="trash"
                disabled={busy}
                onClick={() => void prune()}
                data-testid="module-prune"
              >
                {`clean up · ${formatBytes(state?.reclaimable.bytes ?? 0)}`}
              </Button>
              <Text variant="caption" muted>
                deletes files that nothing installed uses any more.
              </Text>
            </Stack>
          ) : (
            <Text variant="caption" muted data-testid="module-prune-none">
              nothing to clean up: everything here is in use.
            </Text>
          )}
        </SettingsSection>
      ) : null}
      {modal}
      {consent}
    </Page>
  );
}

function Detail({
  name,
  entry,
  status,
  action,
  rows,
  appVersion,
}: {
  name: string;
  appVersion: string | null;
  entry: CatalogEntry | undefined;
  status: ExtensionStatus;
  action: ReactNode;
  rows: PackagesState['packages'];
}) {
  const info = infoFor(name);
  const row = installedRow(name, rows);
  const deps = row ? lockedDependencies(row, rows) : [];
  return (
    <Stack gap={6}>
      <ExtensionHeader
        name={name}
        icon={info.icon}
        description={info.description}
        status={status}
        action={action}
      />
      <SettingsSection
        variant="plain"
        title="about"
        data-testid="extension-facts"
      >
        <FactList
          items={[
            {
              label: 'version',
              value: entry
                ? `v${entry.version}`
                : isBundledModule(name) && appVersion
                  ? `v${appVersion}`
                  : 'installed',
              'data-testid': 'fact-version',
            },
            ...(row ? [{ label: 'installed', value: `v${row.version}` }] : []),
            ...(entry && entry.versions.length > 1
              ? [{ label: 'versions', value: entry.versions.join(', ') }]
              : []),
            {
              label: 'size',
              value: entry
                ? formatBytes(entry.sizeBytes)
                : isBundledModule(name)
                  ? 'built in'
                  : 'on this mac',
            },
            ...(row && !entry
              ? [
                  {
                    label: 'with dependencies',
                    value: formatBytes(
                      row.sizeBytes + deps.reduce((n, d) => n + d.sizeBytes, 0),
                    ),
                    'data-testid': 'fact-install-size',
                  },
                ]
              : []),
            ...(entry
              ? [
                  {
                    label: 'install size',
                    value: `${formatBytes(entry.installSizeBytes)} with everything it needs`,
                    'data-testid': 'fact-install-size',
                  },
                  {
                    label: 'publisher team',
                    value: entry.teamId.toLowerCase(),
                  },
                ]
              : []),
            ...(entry?.minOs
              ? [{ label: 'macos', value: `${entry.minOs} or later` }]
              : []),
          ]}
        />
      </SettingsSection>
      <SettingsSection
        variant="plain"
        title="permissions"
        data-testid="extension-permissions"
      >
        {entry?.permissions.length ? (
          <IconLines aria-label="permissions">
            {entry.permissions.map((p) => (
              <IconLine
                key={p}
                icon={PERMISSIONS[p].icon}
                title={PERMISSIONS[p].label}
                data-testid={`permission-${p}`}
              />
            ))}
          </IconLines>
        ) : (
          <Text variant="caption" muted>
            {entry
              ? 'none it asks for up front. macos asks you the first time it needs one.'
              : 'shared with qrtz: one entry in privacy & security.'}
          </Text>
        )}
        <Text variant="caption" muted>
          apps run inside qrtz and use qrtz's own permissions; there is one qrtz
          entry in privacy & security.
        </Text>
      </SettingsSection>
      <SettingsSection
        variant="plain"
        title="dependencies"
        data-testid="extension-deps"
      >
        {entry?.dependencies.length ? (
          <IconLines aria-label="dependencies">
            {entry.dependencies.map((d) => (
              <IconLine
                key={d.name}
                icon={KIND_ICON[d.kind]}
                title={d.name}
                meta={`${d.kind} · ${d.version ? `v${d.version}` : `no match for ${d.range}`} · ${formatBytes(d.sizeBytes)}${d.direct ? '' : ' · via a dependency'}`}
                data-testid={`dep-${d.name}`}
              />
            ))}
          </IconLines>
        ) : deps.length ? (
          <IconLines
            aria-label="dependencies"
            data-testid={`module-deps-${name}`}
          >
            {deps.map((d) => (
              <IconLine
                key={d.name}
                icon={KIND_ICON[d.kind]}
                title={d.name}
                meta={`${d.kind} · v${d.version} · ${formatBytes(d.sizeBytes)}`}
              />
            ))}
          </IconLines>
        ) : (
          <Text variant="caption" muted>
            none.
          </Text>
        )}
        <Text variant="caption" muted>
          a model or service another app already uses is shared, not downloaded
          twice.
        </Text>
      </SettingsSection>
      <SettingsSection
        variant="plain"
        title="changelog"
        data-testid="extension-changelog"
      >
        {entry?.versions.length ? (
          <IconLines aria-label="versions">
            {entry.versions.map((v) => (
              <IconLine
                key={v}
                icon={v === row?.version ? 'check' : 'circle'}
                title={`v${v}`}
                meta={
                  v === row?.version
                    ? 'installed'
                    : v === entry.version
                      ? 'newest'
                      : undefined
                }
                data-testid={`changelog-${v}`}
              />
            ))}
          </IconLines>
        ) : (
          <Text variant="caption" muted>
            {isBundledModule(name) && appVersion
              ? `v${appVersion}, built into this qrtz.`
              : row
                ? `v${row.version} is installed.`
                : 'no versions listed yet.'}
          </Text>
        )}
      </SettingsSection>
    </Stack>
  );
}
