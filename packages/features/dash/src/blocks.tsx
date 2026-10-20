import { Badge, Link, Progress, Stack, Text } from '@qrtz/ui';
import {
  isHttpsUrl,
  isPunycodeHost,
  parseDashBlock,
  type DashBlockType,
  type DashWidget,
} from '@qrtz/core';

/** A link's real domain, shown next to it (spec: UI defenses). */
export const domainOf = (url: string) => {
  try {
    return new URL(url).hostname;
  } catch {
    return '';
  }
};

/**
 * ADR 0231 finding 3: the rel any Dash link anchor must carry. Dash links render as buttons with no href, so the
 * webview never navigates or opens a window for them; the host opens them in the default browser (no opener, no
 * referrer). Pinned by blocks.links.test.tsx.
 */
export const DASH_LINK_REL = 'noopener noreferrer';

const confirmed = new Set<string>();
/**
 * First click per domain asks first (the interstitial), with a warning for punycode hosts; https only. The host
 * opens it (`dash:open-link`: re-checked in Rust, the Tauri opener); the webview never opens links itself.
 */
export const openLink = async (
  url: string,
  open: (url: string) => Promise<boolean>,
  ask: (msg: string) => boolean = (m) => window.confirm(m),
) => {
  const d = domainOf(url);
  if (!d || !isHttpsUrl(url)) return false;
  if (!confirmed.has(d)) {
    const warn = isPunycodeHost(url)
      ? ' this name uses look-alike letters (xn--) and may imitate another site.'
      : '';
    if (!ask(`open ${d}? an agent put this link here.${warn}`)) return false;
    confirmed.add(d);
  }
  return open(url);
};

const STATUS_TONE = {
  running: 'primary',
  blocked: 'warning',
  waiting_on_user: 'warning',
  done: 'success',
  error: 'danger',
} as const;

/** One widget's body. Agent text is shown as plain text, never as html or markdown. */
export function Block({
  widget,
  open,
}: {
  widget: DashWidget;
  open: (url: string) => Promise<boolean>;
}) {
  const type: DashBlockType = widget.type;
  if (type === 'log' || type === 'chart') {
    const evs = widget.events
      .map((e) => ({ at: e.at, d: parseDashBlock(type, e.data) }))
      .filter((e) => e.d);
    if (!evs.length) return <Text muted>nothing yet</Text>;
    if (type === 'chart') {
      const vals = evs.map((e) => (e.d as { value: number }).value);
      return (
        <Text variant="caption" muted>
          {`last ${vals.at(-1)} · min ${Math.min(...vals)} · max ${Math.max(...vals)} · ${vals.length} points`}
        </Text>
      );
    }
    return (
      <Stack gap={1}>
        {evs.slice(-8).map((e, i) => {
          const d = e.d as { line: string; level?: 'info' | 'warn' | 'error' };
          return (
            <Text
              key={`${e.at}-${i}`}
              variant="caption"
              tone={
                d.level === 'error'
                  ? 'danger'
                  : d.level === 'warn'
                    ? 'warning'
                    : 'muted'
              }
              truncate
            >
              {d.line}
            </Text>
          );
        })}
      </Stack>
    );
  }
  const s = widget.state == null ? null : parseDashBlock(type, widget.state);
  if (!s) return <Text muted>nothing yet</Text>;
  switch (type) {
    case 'status': {
      const v = s as { status: keyof typeof STATUS_TONE; text?: string };
      return (
        <Stack gap={1}>
          <Badge tone={STATUS_TONE[v.status]}>
            {v.status.replace(/_/g, ' ')}
          </Badge>
          {v.text ? <Text>{v.text}</Text> : null}
        </Stack>
      );
    }
    case 'stat': {
      const v = s as {
        value: number;
        label?: string;
        unit?: string;
        delta?: number;
      };
      return (
        <Stack gap={1}>
          <Text variant="h2">{`${v.value}${v.unit ? ` ${v.unit}` : ''}`}</Text>
          {v.label || v.delta != null ? (
            <Text variant="caption" muted>
              {[
                v.label,
                v.delta != null ? `${v.delta >= 0 ? '+' : ''}${v.delta}` : '',
              ]
                .filter(Boolean)
                .join(' · ')}
            </Text>
          ) : null}
        </Stack>
      );
    }
    case 'progress': {
      const v = s as { done: number; total: number; step?: string };
      return (
        <Stack gap={1}>
          <Progress
            value={v.total ? v.done / v.total : 0}
            label={`${v.done} of ${v.total}`}
          />
          <Text variant="caption" muted>
            {`${v.done} of ${v.total}${v.step ? ` · ${v.step}` : ''}`}
          </Text>
        </Stack>
      );
    }
    case 'list': {
      const v = s as {
        items: { text: string; url?: string; state?: string }[];
      };
      return (
        <Stack gap={1}>
          {v.items.slice(0, 10).map((it, i) => (
            <Stack key={i} dir="row" gap={2} align="baseline">
              {it.url ? (
                <Link
                  rel={DASH_LINK_REL}
                  onClick={() => void openLink(it.url as string, open)}
                >
                  {it.text}
                </Link>
              ) : (
                <Text truncate>{it.text}</Text>
              )}
              {it.url ? (
                <Text variant="caption" muted>
                  {domainOf(it.url)}
                </Text>
              ) : null}
              {it.state ? <Badge tone="neutral">{it.state}</Badge> : null}
            </Stack>
          ))}
        </Stack>
      );
    }
    case 'table': {
      const v = s as { columns: string[]; rows: (string | number)[][] };
      return (
        <Stack gap={1}>
          <Text variant="caption" muted truncate>
            {v.columns.join(' · ')}
          </Text>
          {v.rows.slice(0, 10).map((r, i) => (
            <Text key={i} variant="caption" truncate>
              {r.join(' · ')}
            </Text>
          ))}
        </Stack>
      );
    }
    case 'markdown':
      // Stub: shown as plain text (no markdown rendering yet), which is the safest subset.
      return <Text>{(s as { text: string }).text}</Text>;
  }
}
