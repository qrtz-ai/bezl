import type { Meta, StoryObj } from '@storybook/react-vite';
import { NotchSurface, type NotchContent } from '../src';
import { Row } from './_util';

const meta: Meta = { title: 'surfaces/notch surface' };
export default meta;
type S = StoryObj;

function Notch(props: {
  label: string;
  content: NotchContent;
  level?: number;
  source?: string;
  open?: boolean;
  notch?: boolean;
}) {
  const { label, ...rest } = props;
  return (
    <Row label={label}>
      {/* the panel is the real notch's fixed width: it scrolls inside here, never the page */}
      <div style={{ width: '100%', contain: 'inline-size', overflowX: 'auto' }}>
        <NotchSurface {...rest} />
      </div>
    </Row>
  );
}

export const States: S = {
  name: 'states',
  render: () => (
    <>
      <Notch
        label="listening"
        content={{ kind: 'level', busy: false }}
        level={0.6}
      />
      <Notch label="processing" content={{ kind: 'level', busy: true }} />
      <Notch
        label="ctrl status"
        content={{
          kind: 'status',
          text: 'ctrl: open system settings and turn on night shift',
          tone: 'normal',
        }}
      />
      <Notch
        label="error"
        content={{
          kind: 'status',
          text: 'microphone access was turned off',
          tone: 'error',
        }}
      />
      <Notch
        label="module progress"
        source="nano"
        content={{
          kind: 'progress',
          label: 'downloading parakeet',
          value: 0.42,
        }}
      />
      <Notch
        label="no notch (fake tab)"
        source="nano"
        notch={false}
        content={{ kind: 'icon', icon: 'calendar', label: 'standup in 5 min' }}
      />
      <Notch
        label="closed"
        open={false}
        content={{ kind: 'level', busy: false }}
      />
    </>
  ),
};
