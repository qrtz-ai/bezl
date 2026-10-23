import type { Meta, StoryObj } from '@storybook/react-vite';
import {
  Code,
  Heading,
  Icon,
  ICON_NAMES,
  PAGE,
  Page,
  Text,
  ThemeScope,
  TYPE_VARIANTS,
  themeTokens,
  useTheme,
  type IconSize,
  type ThemeMode,
} from '../src';
import { Frame, Row } from './_util';

const meta: Meta = { title: 'foundations' };
export default meta;
type S = StoryObj;

export const Intro: S = {
  name: 'intro',
  render: () => {
    const t = useTheme();
    return (
      <div style={{ maxWidth: 680, display: 'grid', gap: t.space[4] }}>
        <Heading level={1}>qrtz ui</Heading>
        <Text>
          the design system behind qrtz, built in public. every component here
          is the one the app ships, from @qrtz/ui.
        </Text>
        <Text muted>
          one font (geist pixel square), feather icons, thin borders instead of
          shadows, small radii, fast ease-out motion. dark is the default; use
          the theme switch in the toolbar to see light.
        </Text>
        <Text muted>
          all copy is lowercase. the word is opts, never the other one.
        </Text>
        <Row label="both themes">
          {(['dark', 'light'] as ThemeMode[]).map((m) => (
            <ThemeScope
              key={m}
              mode={m}
              style={{
                padding: 16,
                borderRadius: 8,
                border: '1px solid #404040',
              }}
            >
              <Text>{m}</Text>
            </ThemeScope>
          ))}
        </Row>
      </div>
    );
  },
};

export const Type: S = {
  name: 'type scale',
  render: () => {
    const t = useTheme();
    return (
      <div style={{ display: 'grid', gap: t.space[3] }}>
        <Text muted>geist pixel square, the only font.</Text>
        {TYPE_VARIANTS.map((v) => (
          <div
            key={v}
            style={{
              display: 'grid',
              gridTemplateColumns: '96px minmax(0, 1fr) 180px',
              gap: t.space[4],
              alignItems: 'baseline',
            }}
          >
            <Code>{v}</Code>
            <Text variant={v} truncate>
              {v === 'code' || v === 'mono'
                ? '00:12:34 · note.record()'
                : 'meeting notes'}
            </Text>
            <Text variant="caption" muted>
              {t.type[v].size} / {t.type[v].lineHeight}
              {t.type[v].tracking !== '0em' ? ` / ${t.type[v].tracking}` : ''}
            </Text>
          </div>
        ))}
      </div>
    );
  },
};

function Swatches({ mode }: { mode: ThemeMode }) {
  const t = themeTokens(mode);
  return (
    <ThemeScope mode={mode} style={{ padding: 16, borderRadius: 8 }}>
      <Text variant="caption" muted>
        {mode}
      </Text>
      <div style={{ display: 'flex', flexWrap: 'wrap', gap: 12, marginTop: 8 }}>
        {Object.entries(t.color)
          .filter(([k]) => k !== 'accentText')
          .map(([k, v]) => (
            <div key={k} style={{ display: 'grid', gap: 4, width: 96 }}>
              <div
                style={{
                  height: 40,
                  borderRadius: 6,
                  background: v,
                  border: `1px solid ${t.color.border}`,
                }}
              />
              <Text variant="caption" truncate>
                {k}
              </Text>
              <Text variant="caption" muted>
                {v.toLowerCase()}
              </Text>
            </div>
          ))}
      </div>
    </ThemeScope>
  );
}

export const Colors: S = {
  name: 'colors',
  render: () => (
    <div style={{ display: 'grid', gap: 16 }}>
      <Swatches mode="dark" />
      <Swatches mode="light" />
    </div>
  ),
};

export const Tokens: S = {
  name: 'spacing, radius, motion',
  render: () => {
    const t = useTheme();
    return (
      <>
        <Row label="space">
          {Object.entries(t.space).map(([k, v]) => (
            <div
              key={k}
              style={{ display: 'grid', gap: 4, justifyItems: 'center' }}
            >
              <div
                style={{
                  width: v,
                  minWidth: 1,
                  height: 16,
                  background: t.color.textMuted,
                  borderRadius: 2,
                }}
              />
              <Text variant="caption" muted>
                {k} · {v}
              </Text>
            </div>
          ))}
        </Row>
        <Row label="radius">
          {Object.entries(t.radius).map(([k, v]) => (
            <div
              key={k}
              style={{ display: 'grid', gap: 4, justifyItems: 'center' }}
            >
              <div
                style={{
                  width: 40,
                  height: 40,
                  border: `1px solid ${t.color.borderStrong}`,
                  borderRadius: v,
                }}
              />
              <Text variant="caption" muted>
                {k} · {v}
              </Text>
            </div>
          ))}
        </Row>
        <Row label="motion">
          <div style={{ display: 'grid', gap: 4 }}>
            {Object.entries(t.motion).map(([k, v]) => (
              <Text key={k} variant="caption">
                {k} · <Code>{String(v)}</Code>
              </Text>
            ))}
          </div>
        </Row>
      </>
    );
  },
};

const SIZES: IconSize[] = ['xs', 'sm', 'md', 'lg', 'xl'];
export const Icons: S = {
  name: 'icons (feather)',
  render: () => {
    const t = useTheme();
    return (
      <>
        <Row label="sizes">
          {SIZES.map((s) => (
            <span
              key={s}
              style={{ display: 'grid', justifyItems: 'center', gap: 4 }}
            >
              <Icon name="mic" size={s} />
              <Text variant="caption" muted>
                {s} · {t.icon.size[s]}
              </Text>
            </span>
          ))}
        </Row>
        <Row label="every icon in use">
          <div
            style={{
              display: 'grid',
              gridTemplateColumns: 'repeat(auto-fill, minmax(140px, 1fr))',
              gap: t.space[3],
              width: '100%',
            }}
          >
            {ICON_NAMES.map((n) => (
              <div
                key={n}
                style={{
                  display: 'flex',
                  alignItems: 'center',
                  gap: t.space[2],
                  padding: t.space[2],
                  border: `1px solid ${t.color.border}`,
                  borderRadius: t.radius.md,
                  minWidth: 0,
                }}
              >
                <Icon name={n} />
                <Text variant="caption" muted truncate>
                  {n}
                </Text>
              </div>
            ))}
          </div>
        </Row>
      </>
    );
  },
};

export const PageLayout: S = {
  name: 'page layout',
  render: () => {
    const t = useTheme();
    const guide = `1px dashed ${t.color.warning}`;
    return (
      <>
        <Text muted>
          every page: a {PAGE.maxWidth}px column, no title row, {PAGE.gutter}px
          gutters, {PAGE.padTop}px top padding and {PAGE.gap}px between
          sections.
        </Text>
        <div style={{ height: 16 }} />
        <Frame height={420}>
          <Page>
            <div style={{ outline: guide, padding: 12 }}>
              <Text>section one</Text>
            </div>
            <div style={{ outline: guide, padding: 12 }}>
              <Text>section two, {PAGE.gap}px below</Text>
            </div>
            <div style={{ outline: guide, padding: 12 }}>
              <Text>section three</Text>
            </div>
          </Page>
        </Frame>
        <Row label="tokens">
          <div style={{ display: 'grid', gap: 4 }}>
            {Object.entries(PAGE).map(([k, v]) => (
              <Text key={k} variant="caption">
                {k} · {v}px
              </Text>
            ))}
          </div>
        </Row>
      </>
    );
  },
};
