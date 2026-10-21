import type { Meta, StoryObj } from '@storybook/react-vite';
import { LongTextSample, ThemeProvider, ToastProvider } from '@qrtz/ui';

/**
 * Overflow check (QRT-304): @qrtz/ui's long-text state (its public LongTextSample, the same one the
 * components/states story renders), shown here so the e2e run can assert nothing spills sideways. screenshots/pages.spec.ts checks layout for `checks--*` stories and does not screenshot them.
 */
const meta: Meta = { title: 'checks' };
export default meta;

export const LongText: StoryObj = {
  name: 'long text',
  render: () => (
    <ThemeProvider mode="dark">
      <ToastProvider>
        <LongTextSample />
      </ToastProvider>
    </ThemeProvider>
  ),
};
