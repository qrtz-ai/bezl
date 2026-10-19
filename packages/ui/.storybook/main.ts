import type { StorybookConfig } from '@storybook/react-vite';

const config: StorybookConfig = {
  framework: { name: '@storybook/react-vite', options: {} },
  stories: ['../stories/**/*.mdx', '../stories/**/*.stories.tsx'],
  addons: [],
  staticDirs: ['./public'],
  core: { disableTelemetry: true },
  async viteFinal(cfg) {
    const { default: react } = await import('@vitejs/plugin-react');
    cfg.plugins = [...(cfg.plugins ?? []), react()];
    return cfg;
  },
};

export default config;
