import { typeStyle } from './tokens';
import type { ReactNode } from 'react';
import { useTheme } from './theme';
import { Text, LongText } from './Text';
import { Button } from './Button';
import { Badge } from './Badge';
import { ListItem } from './Overlay';
import { ErrorCallout, userError } from './UserError';

/**
 * The long-text state (QRT-304): text, an unbroken word, a button and badge, a list item, an error and code, each
 * far longer than its 320px frame. The `components/states` story and the desktop app's overflow check both render
 * this, so they can't drift apart. Nothing in it may spill sideways.
 */
export const LONG_SAMPLE =
  'a very long name that keeps going well past the width of its container so we can see it wrap or truncate without pushing anything sideways: '.repeat(
    3,
  );
export const LONG_WORD_SAMPLE = 'supercalifragilisticexpialidocious'.repeat(6);

function SampleRow({
  label,
  children,
}: {
  label: string;
  children: ReactNode;
}) {
  const t = useTheme();
  return (
    <div style={{ display: 'grid', gap: t.space[2], marginBottom: t.space[5] }}>
      <span style={{ color: t.color.textMuted, ...typeStyle('caption', t) }}>
        {label}
      </span>
      <div
        style={{
          display: 'flex',
          flexWrap: 'wrap',
          gap: t.space[3],
          alignItems: 'center',
          minWidth: 0,
        }}
      >
        {children}
      </div>
    </div>
  );
}

export function LongTextSample({ width = 320 }: { width?: number }) {
  return (
    <div style={{ width, maxWidth: '100%' }} data-testid="long-text-sample">
      <SampleRow label="text">
        <Text>{LONG_SAMPLE}</Text>
      </SampleRow>
      <SampleRow label="unbroken word">
        <Text>{LONG_WORD_SAMPLE}</Text>
      </SampleRow>
      <SampleRow label="button and badge">
        <Button>{LONG_SAMPLE.slice(0, 80)}</Button>
        <Badge>{LONG_SAMPLE.slice(0, 60)}</Badge>
      </SampleRow>
      <SampleRow label="list item">
        <ListItem right={<Badge>new</Badge>}>{LONG_SAMPLE}</ListItem>
      </SampleRow>
      <SampleRow label="error">
        <ErrorCallout error={userError(LONG_SAMPLE, 'load the long thing')} />
      </SampleRow>
      <SampleRow label="code">
        <LongText>{LONG_WORD_SAMPLE}</LongText>
      </SampleRow>
    </div>
  );
}
