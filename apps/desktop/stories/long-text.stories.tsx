import type { Meta, StoryObj } from '@storybook/react-vite';
import { ThemeProvider, ToastProvider } from '@qrtz/ui';
import { LongTextState } from '../../../packages/ui/stories/states.stories';

/**
 * Overflow check (QRT-304): @qrtz/ui's long-text state, rendered here so the e2e run can assert nothing spills
 * sideways. screenshots/pages.spec.ts checks layout for `checks--*` stories and does not screenshot them.
 */
const meta: Meta = { title: 'checks' };
export default meta;

const render = LongTextState.render as () => React.ReactNode;

export const LongText: StoryObj = {
  name: 'long text',
  render: () => (
    <ThemeProvider mode="dark">
      <ToastProvider>{render()}</ToastProvider>
    </ThemeProvider>
  ),
};
