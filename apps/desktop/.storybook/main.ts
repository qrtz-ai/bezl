import type { StorybookConfig } from '@storybook/react-vite';

// Page stories (QRT-304): the real feature views behind the desktop mock backend. They live here, not in
// @qrtz/ui, so the dependency direction stays app → ui (ui never imports a feature view).
const config: StorybookConfig = {
  framework: { name: '@storybook/react-vite', options: {} },
  stories: ['../stories/**/*.stories.tsx'],
  addons: [],
  core: { disableTelemetry: true },
  // Tailwind (@qrtz/ui's shadcn styles) comes from ../vite.config.mts, which Storybook's Vite builder loads.
};

export default config;
