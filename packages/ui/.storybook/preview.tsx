import type { Decorator, Preview } from '@storybook/react-vite';
import { ThemeProvider, ToastProvider } from '../src';

const withTheme: Decorator = (Story, ctx) => {
  const mode = ctx.globals.theme === 'light' ? 'light' : 'dark';
  return (
    <ThemeProvider mode={mode}>
      <ToastProvider>
        <div style={{ padding: 24, maxWidth: '100%', overflowX: 'hidden' }}>
          <Story />
        </div>
      </ToastProvider>
    </ThemeProvider>
  );
};

const preview: Preview = {
  decorators: [withTheme],
  globalTypes: {
    theme: {
      description: 'qrtz theme',
      toolbar: {
        title: 'theme',
        icon: 'mirror',
        items: [
          { value: 'dark', title: 'dark' },
          { value: 'light', title: 'light' },
        ],
        dynamicTitle: true,
      },
    },
  },
  initialGlobals: { theme: 'dark' },
  parameters: {
    layout: 'fullscreen',
    controls: { expanded: true },
    options: {
      storySort: { order: ['intro', 'foundations', 'components', 'surfaces'] },
    },
  },
};

export default preview;
