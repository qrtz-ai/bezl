import type { Meta, StoryObj } from '@storybook/react-vite';
import { PlainList, Stack, TalkHistoryRow, ThemeProvider } from '@qrtz/ui';

/**
 * ContextMenu on shadcn ContextMenu, on an echo history entry: click copies, right-click opens copy and delete.
 * screenshots/primitives.spec.ts right-clicks it open, screenshots it and runs axe.
 */
const meta: Meta = { title: 'primitives/context menu' };
export default meta;

const noop = () => {};

export const EchoEntry: StoryObj = {
  name: 'echo entry',
  render: () => (
    <ThemeProvider mode="dark">
      <Stack pad={4}>
        <PlainList label="dictation history">
          <TalkHistoryRow
            text="remind me to send the quarterly numbers to the team before friday"
            meta="2 min ago · 14 words"
            onCopy={noop}
            onDelete={noop}
          />
          <TalkHistoryRow
            text="draft a reply saying thursday works"
            meta="1 h ago · 6 words"
            onCopy={noop}
            onDelete={noop}
          />
        </PlainList>
      </Stack>
    </ThemeProvider>
  ),
};
