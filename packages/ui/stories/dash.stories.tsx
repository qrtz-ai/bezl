import type { Meta, StoryObj } from '@storybook/react-vite';
import { Button, Stack, Text } from '../src';
import { Row, noop } from './_util';

/** QRT-298: dash's single copy-prompt action. Rust issues the code and copies the prompt; the view never sees it. */
const meta: Meta = { title: 'components/dash' };
export default meta;
type S = StoryObj;

const CopyPrompt = ({ state }: { state: 'ready' | 'copied' | 'offline' }) => (
  <Stack dir="row" gap={2} align="center">
    {state === 'offline' ? (
      <Text variant="caption" muted>
        not connected
      </Text>
    ) : state === 'copied' ? (
      <Text variant="caption" muted>
        copied
      </Text>
    ) : null}
    <Button
      size="sm"
      variant="secondary"
      icon="copy"
      onClick={noop}
      disabled={state === 'offline'}
    >
      copy prompt
    </Button>
  </Stack>
);

export const CopyPromptStates: S = {
  render: () => (
    <Stack gap={3}>
      <Row label="connected">
        <CopyPrompt state="ready" />
      </Row>
      <Row label="after copy">
        <CopyPrompt state="copied" />
      </Row>
      <Row label="no origin">
        <CopyPrompt state="offline" />
      </Row>
    </Stack>
  ),
};
