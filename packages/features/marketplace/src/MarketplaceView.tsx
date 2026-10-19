import {
  useCallback,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from 'react';
import {
  Button,
  Callout,
  Dialog,
  EmptyState,
  ExtensionCard,
  ExtensionGrid,
  ExtensionHeader,
  FactList,
  Heading,
  IconButton,
  IconLine,
  IconLines,
  InstallProgress,
  ModuleRow,
  Page,
  SettingsSection,
  Stack,
  Tabs,
  Text,
  Toolbar,
  formatBytes,
  useToast,
  type ExtensionStatus,
} from '@qrtz/ui';
import {
  isModuleOn,
  loadConfig,
  requestNavigate,
  type CatalogEntry,
  type Config,
  type PackageOp,
  type PackagePlan,
  type PackagesProgress,
  type PackagesState,
} from '@qrtz/core';
import { useQrtz, useSubAppRoute } from '@qrtz/core/react';
import {
  KIND_ICON,
  PERMISSIONS,
  infoFor,
  installedRow,
  isModuleId,
  listings,
  lockedDependencies,
  marketPath,
  parseMarketRoute,
  sizeLine,
  statusOf,
  type MarketRoute,
} from './catalog';

type Verb = 'install' | 'update' | 'remove';
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
 * ADR 0161: the marketplace. Browse (an app-store grid of extensions from the stored signed index), a detail page
 * per extension, and the module manager (`installed`). Opening it never goes online: "check for extensions" and an
 * install the user confirms are the only network, and an install shows its plan (what is added, removed and
 * downloaded) before anything moves.
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
  const [error, setError] = useState<string | null>(null);
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

  const fail = useCallback((e: Error) => setError(e.message), []);
  const load = useCallback(
    () => qrtz.invoke('packages:list').then(setState, fail),
    [qrtz, fail],
  );
  useEffect(() => {
    void load();
    const apply = (c: Record<string, unknown>) => setCfg(loadConfig(c));
    qrtz.invoke('config:get').then(apply, () => {});
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
    working?.name === name ? 'working' : statusOf(name, entryOf(name), rows);

  const check = () => {
    setError(null);
    setChecking(true);
    qrtz
      .invoke('packages:refresh')
      .then(setState, fail)
      .finally(() => setChecking(false));
  };
  const propose = (name: string, verb: Verb) => {
    setError(null);
    const op: PackageOp =
      verb === 'install'
        ? { op: 'install', names: [name] }
        : verb === 'update'
          ? { op: 'update', names: [name], latest: true }
          : { op: 'remove', names: [name] };
    qrtz
      .invoke('packages:plan', op)
      .then((plan) => setPending({ name, verb, plan }), fail);
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
      .then(() => {
        toast(`${p.name} ${PAST[p.verb]}`, 'success');
        return load();
      }, fail)
      .finally(() => {
        setWorking(null);
        setProgress(null);
      });
  };
  const prune = () =>
    qrtz
      .invoke('packages:prune')
      .then(
        (r) =>
          toast(
            r.dirs || r.blobs
              ? `freed ${formatBytes(r.bytes)}`
              : 'nothing to clean up',
            'success',
          ),
        fail,
      );
  const setEnabled = (name: string, on: boolean) => {
    if (!isModuleId(name)) return;
    qrtz
      .invoke('packages:set-enabled', { name, enabled: on })
      .then(() => qrtz.invoke('config:get'))
      .then((c) => setCfg(loadConfig(c)), fail);
  };
  const enabled = (name: string) =>
    isModuleId(name) && cfg ? isModuleOn(cfg, name) : true;

  const pct =
    progress && progress.totalBytes
      ? progress.doneBytes / progress.totalBytes
      : 0;
  const busy = working !== null || pending !== null;

  const primary = (name: string, size: 'sm' | 'md' = 'sm') => {
    const s = statusFor(name);
    const view = infoFor(name).view;
    if (s === 'working')
      return (
        <Button size={size} loading disabled>
          {ING[working?.verb ?? 'install']}
        </Button>
      );
    if (s === 'available')
      return (
        <Button
          size={size}
          icon="download"
          disabled={busy}
          onClick={() => propose(name, 'install')}
          data-testid={`extension-install-${name}`}
        >
          install
        </Button>
      );
    if (s === 'update')
      return (
        <Button
          size={size}
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
        size={size}
        variant="secondary"
        disabled={!enabled(name)}
        onClick={() => requestNavigate({ app: view })}
        data-testid={`extension-launch-${name}`}
      >
        open
      </Button>
    ) : null;
  };

  const tabs = (
    <Tabs
      tabs={[
        { id: 'browse', label: 'browse' },
        { id: 'installed', label: 'installed' },
      ]}
      value={at.page === 'installed' ? 'installed' : 'browse'}
      onChange={(id) => go({ page: id })}
    />
  );

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

  const errorBox = error ? (
    <Callout
      tone="danger"
      title={error}
      data-testid="marketplace-error"
      action={
        <Button variant="secondary" size="sm" onClick={() => setError(null)}>
          dismiss
        </Button>
      }
    />
  ) : null;

  const installBar =
    working && working.verb !== 'remove' ? (
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

  if (at.page === 'item')
    return (
      <Page width={760} data-testid="marketplace-item">
        <Toolbar>
          <IconButton
            icon="back"
            label="back to marketplace"
            onClick={() => go({ page: 'browse' })}
            data-testid="marketplace-back"
          />
        </Toolbar>
        {errorBox}
        <Detail
          name={at.name}
          entry={entryOf(at.name)}
          status={statusFor(at.name)}
          action={primary(at.name, 'md')}
          rows={rows}
          busy={busy}
          onRemove={() => propose(at.name, 'remove')}
        />
        {working?.name === at.name ? installBar : null}
        {consent}
      </Page>
    );

  if (at.page === 'installed') {
    const names = [
      ...new Set([
        ...rows.filter((r) => r.kind === 'extension').map((r) => r.name),
        ...listings(available)
          .map((l) => l.name)
          .filter((n) => statusOf(n, entryOf(n), rows) === 'built-in'),
      ]),
    ];
    const shared = rows.filter((r) => r.kind !== 'extension');
    return (
      <Page width={760} data-testid="module-manager">
        <Heading>marketplace</Heading>
        {tabs}
        {errorBox}
        {installBar}
        <SettingsSection title="extensions" data-testid="module-list">
          {names.length ? null : (
            <EmptyState icon="package" title="no extensions installed" />
          )}
          {names.map((name) => {
            const info = infoFor(name);
            const row = installedRow(name, rows);
            const entry = entryOf(name);
            const deps = row ? lockedDependencies(row, rows) : [];
            const s = statusFor(name);
            const on = enabled(name);
            return (
              <ModuleRow
                key={name}
                name={name}
                icon={info.icon}
                status={s}
                meta={
                  row
                    ? `v${row.version} · ${formatBytes(row.sizeBytes + deps.reduce((n, d) => n + d.sizeBytes, 0))}${row.disabled ? ` · switched off: ${row.disabled}` : ''}`
                    : 'built into this qrtz'
                }
                enabled={on}
                toggleDisabled={!isModuleId(name) || !!row?.disabled}
                onToggle={(v) => setEnabled(name, v)}
                actions={
                  <>
                    {s === 'update' ? primary(name) : null}
                    {s === 'built-in' && entry ? (
                      <Button
                        size="sm"
                        variant="secondary"
                        icon="download"
                        disabled={busy}
                        onClick={() => propose(name, 'install')}
                        data-testid={`module-get-${name}`}
                      >
                        {`get v${entry.version}`}
                      </Button>
                    ) : null}
                    {info.view ? (
                      <Button
                        size="sm"
                        variant="ghost"
                        icon="settings"
                        disabled={!on}
                        onClick={() =>
                          info.view &&
                          requestNavigate({ app: info.view, settings: true })
                        }
                        data-testid={`module-settings-${name}`}
                      >
                        settings
                      </Button>
                    ) : null}
                    <Button
                      size="sm"
                      variant="ghost"
                      onClick={() => go({ page: 'item', name })}
                      data-testid={`module-details-${name}`}
                    >
                      details
                    </Button>
                    {row ? (
                      <Button
                        size="sm"
                        variant="ghost"
                        icon="trash"
                        disabled={busy}
                        onClick={() => propose(name, 'remove')}
                        data-testid={`module-remove-${name}`}
                      >
                        remove
                      </Button>
                    ) : null}
                  </>
                }
              >
                {deps.length ? (
                  <IconLines
                    aria-label={`${name} dependencies`}
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
                ) : null}
              </ModuleRow>
            );
          })}
        </SettingsSection>
        <SettingsSection title="shared" data-testid="module-shared">
          {shared.length ? (
            <IconLines aria-label="shared packages">
              {shared.map((d) => (
                <IconLine
                  key={`${d.name}@${d.version}`}
                  icon={KIND_ICON[d.kind]}
                  title={d.name}
                  meta={`${formatBytes(d.sizeBytes)} · used by ${d.requiredBy.map((x) => x.split('@')[0]).join(', ') || 'nothing'}`}
                />
              ))}
            </IconLines>
          ) : (
            <Text variant="caption" muted>
              no shared services or models installed.
            </Text>
          )}
          <Stack dir="row" gap={2} align="center">
            <Button
              size="sm"
              variant="secondary"
              icon="trash"
              disabled={busy}
              onClick={() => void prune()}
              data-testid="module-prune"
            >
              clean up
            </Button>
            <Text variant="caption" muted>
              deletes files that nothing installed uses any more.
            </Text>
          </Stack>
        </SettingsSection>
        {consent}
      </Page>
    );
  }

  return (
    <Page width={760} data-testid="marketplace">
      <Heading>marketplace</Heading>
      {tabs}
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
            check for extensions
          </Button>
        }
      >
        <Text variant="caption" muted data-testid="marketplace-index">
          {state?.index
            ? `signed list v${state.index.version} · checked ${new Date(state.index.issuedMs).toLocaleDateString().toLowerCase()}`
            : 'checking goes online to modules.qrtz.ai for the signed list. opening this page never does.'}
        </Text>
      </Toolbar>
      {errorBox}
      {installBar}
      <ExtensionGrid data-testid="marketplace-grid">
        {listings(available).map(({ name, entry }) => {
          const info = infoFor(name);
          return (
            <ExtensionCard
              key={name}
              name={name}
              icon={info.icon}
              description={info.description}
              version={entry?.version ?? 'built in'}
              size={sizeLine(entry)}
              status={statusFor(name)}
              progress={working?.name === name ? pct : null}
              action={primary(name)}
              onOpen={() => go({ page: 'item', name })}
            />
          );
        })}
      </ExtensionGrid>
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
  busy,
  onRemove,
}: {
  name: string;
  entry: CatalogEntry | undefined;
  status: ExtensionStatus;
  action: ReactNode;
  rows: PackagesState['packages'];
  busy: boolean;
  onRemove: () => void;
}) {
  const info = infoFor(name);
  const row = installedRow(name, rows);
  return (
    <Stack gap={6}>
      <ExtensionHeader
        name={name}
        icon={info.icon}
        description={info.description}
        status={status}
        action={action}
      />
      <SettingsSection title="about" data-testid="extension-facts">
        <FactList
          items={[
            {
              label: 'version',
              value: entry ? `v${entry.version}` : 'built in',
              'data-testid': 'fact-version',
            },
            ...(row ? [{ label: 'installed', value: `v${row.version}` }] : []),
            ...(entry && entry.versions.length > 1
              ? [{ label: 'versions', value: entry.versions.join(', ') }]
              : []),
            {
              label: 'size',
              value: entry ? formatBytes(entry.sizeBytes) : 'included in qrtz',
            },
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
      <SettingsSection title="permissions" data-testid="extension-permissions">
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
          extensions run inside qrtz and use qrtz's own permissions; there is
          one qrtz entry in privacy & security.
        </Text>
      </SettingsSection>
      <SettingsSection title="dependencies" data-testid="extension-deps">
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
        ) : (
          <Text variant="caption" muted>
            none.
          </Text>
        )}
        <Text variant="caption" muted>
          a model or service another extension already uses is shared, not
          downloaded twice.
        </Text>
      </SettingsSection>
      {row ? (
        <Stack dir="row">
          <Button
            variant="ghost"
            icon="trash"
            disabled={busy}
            onClick={onRemove}
            data-testid={`extension-remove-${name}`}
          >
            remove
          </Button>
        </Stack>
      ) : null}
    </Stack>
  );
}
