import { useCallback, useEffect, useState } from 'react';
import {
  Badge,
  Button,
  Callout,
  Card,
  CardGrid,
  EmptyState,
  Field,
  GridCard,
  IconButton,
  Input,
  Mono,
  Page,
  SettingsSection,
  Stack,
  Text,
} from '@qrtz/ui';
import {
  DASH_ORIGIN,
  type DashAgent,
  type DashDelete,
  type DashView as DashViewData,
} from '@qrtz/core';
import { useQrtz } from '@qrtz/core/react';
import { Block } from './blocks';

const POLL_MS = 5000;

const ago = (ms: number) => {
  const m = Math.round(ms / 60000);
  return m < 1
    ? 'just now'
    : m < 60
      ? `${m} min ago`
      : `${Math.round(m / 60)} h ago`;
};

/**
 * Agent Dash (ADR 0228): a strictly read-only view of what your agents report. The only controls that reach an
 * agent are the deletes (it sees a 410); approving a new agent is account security, not a message to it.
 */
export default function DashView() {
  const qrtz = useQrtz();
  const [view, setView] = useState<DashViewData | null>(null);
  const [code, setCode] = useState<{ code: string; expiresAt: number } | null>(
    null,
  );
  const [confirmDash, setConfirmDash] = useState(false);
  const [setupToken, setSetupToken] = useState('');
  const [serverError, setServerError] = useState<string | null>(null);
  const load = useCallback(
    () => void qrtz.invoke('dash:view').then(setView, () => setView(null)),
    [qrtz],
  );
  useEffect(() => {
    load();
    const id = setInterval(load, POLL_MS);
    return () => clearInterval(id);
  }, [load]);
  const del = (d: DashDelete) =>
    void qrtz.invoke('dash:delete', d).then(load, () => {});
  const review = (agentId: string, approve: boolean) =>
    void qrtz
      .invoke('dash:review-agent', { agentId, approve })
      .then(load, () => {});
  const open = (url: string) =>
    qrtz.invoke('dash:open-link', { url }).then(
      (r) => r.opened,
      () => false,
    );
  const issue = () =>
    void qrtz.invoke('dash:issue-code').then(setCode, () => setCode(null));

  // ADR 0231: the setup token is sent once and dropped; the owner key it buys stays in the keychain (rust side).
  const connect = () =>
    void qrtz.invoke('dash:connect', { setupToken }).then(
      () => {
        setSetupToken('');
        setServerError(null);
        load();
      },
      () => setServerError("couldn't connect. check the setup token."),
    );
  // ADR 0231 amendment 1: no token = the dashboard the helper already holds a key for.
  const reconnect = () =>
    void qrtz.invoke('dash:connect', {}).then(
      () => {
        setServerError(null);
        load();
      },
      () => setServerError('no saved server yet. use a setup token.'),
    );
  const disconnect = () =>
    void qrtz.invoke('dash:disconnect').then(load, () => {});

  const pending = view?.agents.filter((a) => a.status === 'pending') ?? [];
  const live = view?.agents.filter((a) => a.status === 'approved') ?? [];
  return (
    <Page
      data-testid="dash"
      title="dash"
      actions={
        <>
          <Text variant="caption" muted truncate>
            what your agents are doing, read-only
          </Text>
          {view && !view.deleted ? (
            <Button
              size="sm"
              variant="secondary"
              icon="download"
              onClick={issue}
              data-testid="dash-add-agent"
            >
              add an agent
            </Button>
          ) : null}
        </>
      }
    >
      {view?.deleted ? (
        <Callout tone="info" title="this dashboard was deleted">
          agents writing to it are told to stop.
        </Callout>
      ) : null}
      {code ? (
        <Callout
          tone="info"
          title="enrollment code"
          data-testid="dash-code"
          action={
            <Button size="sm" variant="ghost" onClick={() => setCode(null)}>
              done
            </Button>
          }
        >
          <Stack gap={1}>
            <Mono>{code.code}</Mono>
            <Text variant="caption" muted>
              {`give it to one agent with ${DASH_ORIGIN}/v1/readme. works once, for 15 minutes. it isn't shown again.`}
            </Text>
          </Stack>
        </Callout>
      ) : null}
      {pending.map((a) => (
        <Callout
          key={a.id}
          tone="warning"
          title={`new agent: ${a.name}`}
          data-testid={`dash-pending-${a.id}`}
          action={
            <Stack dir="row" gap={2}>
              <Button size="sm" onClick={() => review(a.id, true)}>
                approve
              </Button>
              <Button
                size="sm"
                variant="ghost"
                onClick={() => review(a.id, false)}
              >
                reject
              </Button>
            </Stack>
          }
        >
          <Text variant="caption" muted>
            {`fingerprint ${a.fingerprint}. approve only if it matches what your agent told you.`}
          </Text>
        </Callout>
      ))}
      {view && !view.deleted && !live.length && !pending.length ? (
        <EmptyState icon="grid" title="no agents yet" />
      ) : null}
      {live.map((a) => (
        <AgentSection
          key={a.id}
          agent={a}
          now={Date.now()}
          onDelete={del}
          open={open}
        />
      ))}
      {view ? (
        <SettingsSection title="server">
          {view.connected ? (
            <Stack dir="row" gap={2} align="center">
              <Text variant="caption" muted data-testid="dash-connected">
                {`connected to ${view.origin}. agents can reach this dashboard.`}
              </Text>
              <Button size="sm" variant="ghost" onClick={disconnect}>
                disconnect
              </Button>
            </Stack>
          ) : (
            <Stack gap={2}>
              <Text variant="caption" muted>
                {`local only: agents can't reach it. connect to ${view.origin} with its setup token.`}
              </Text>
              <Stack dir="row" gap={2} align="end">
                <Field label="setup token">
                  <Input
                    type="password"
                    autoComplete="off"
                    value={setupToken}
                    onChange={(e) => setSetupToken(e.target.value)}
                    data-testid="dash-setup-token"
                  />
                </Field>
                <Button
                  size="sm"
                  variant="secondary"
                  disabled={!setupToken.trim()}
                  onClick={connect}
                  data-testid="dash-connect"
                >
                  connect
                </Button>
                <Button
                  size="sm"
                  variant="ghost"
                  onClick={reconnect}
                  data-testid="dash-reconnect"
                >
                  reconnect
                </Button>
              </Stack>
              {serverError ? (
                <Text variant="caption" tone="danger">
                  {serverError}
                </Text>
              ) : null}
            </Stack>
          )}
        </SettingsSection>
      ) : null}
      {view && !view.deleted ? (
        <SettingsSection title="danger zone">
          <Stack dir="row" gap={2} align="center">
            <Text variant="caption" muted>
              {`history is kept ${view.retentionDays} days.`}
            </Text>
            {confirmDash ? (
              <>
                <Button
                  size="sm"
                  variant="danger"
                  onClick={() => {
                    setConfirmDash(false);
                    del({ scope: 'dashboard' });
                  }}
                  data-testid="dash-delete-confirm"
                >
                  delete everything
                </Button>
                <Button
                  size="sm"
                  variant="ghost"
                  onClick={() => setConfirmDash(false)}
                >
                  cancel
                </Button>
              </>
            ) : (
              <Button
                size="sm"
                variant="ghost"
                icon="trash"
                onClick={() => setConfirmDash(true)}
                data-testid="dash-delete"
              >
                delete dashboard
              </Button>
            )}
          </Stack>
        </SettingsSection>
      ) : null}
    </Page>
  );
}

function AgentSection({
  agent: a,
  now,
  onDelete,
  open,
}: {
  agent: DashAgent;
  now: number;
  onDelete: (d: DashDelete) => void;
  open: (url: string) => Promise<boolean>;
}) {
  return (
    <Card padding={4} data-testid={`dash-agent-${a.id}`}>
      <Stack gap={3}>
        {/* Server-assigned identity, in chrome the agent can't style. */}
        <Stack dir="row" gap={2} align="center" justify="between">
          <Stack dir="row" gap={2} align="center">
            <Text variant="h3">{a.name}</Text>
            <Mono>{a.fingerprint}</Mono>
            {a.stale ? (
              <Badge tone="warning" data-testid="dash-stale">
                stale
              </Badge>
            ) : a.heartbeat ? (
              <Badge tone={a.heartbeat === 'running' ? 'success' : 'neutral'}>
                {a.heartbeat}
              </Badge>
            ) : null}
            {a.flagged ? (
              <Badge tone="danger">sent a secret, redacted</Badge>
            ) : null}
          </Stack>
          <Stack dir="row" gap={2} align="center">
            <Text variant="caption" muted>
              {ago(now - a.lastSeen)}
            </Text>
            <IconButton
              icon="trash"
              label={`delete ${a.name}`}
              onClick={() => onDelete({ scope: 'agent', agentId: a.id })}
            />
          </Stack>
        </Stack>
        {a.widgets.length ? (
          <CardGrid>
            {a.widgets.map((w) => (
              <GridCard key={w.key} data-testid={`dash-widget-${w.key}`}>
                <Stack gap={2}>
                  <Stack dir="row" gap={2} align="center" justify="between">
                    <Text variant="label" truncate>
                      {w.title}
                    </Text>
                    <IconButton
                      icon="close"
                      label={`delete ${w.title}`}
                      onClick={() =>
                        onDelete({ scope: 'widget', agentId: a.id, key: w.key })
                      }
                    />
                  </Stack>
                  <Block widget={w} open={open} />
                </Stack>
              </GridCard>
            ))}
          </CardGrid>
        ) : (
          <Text variant="caption" muted>
            no widgets yet
          </Text>
        )}
      </Stack>
    </Card>
  );
}
