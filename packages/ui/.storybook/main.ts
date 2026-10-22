import type { StorybookConfig } from '@storybook/react-vite';

const config: StorybookConfig = {
  framework: { name: '@storybook/react-vite', options: {} },
  stories: ['../stories/**/*.mdx', '../stories/**/*.stories.tsx'],
  addons: [],
  staticDirs: ['./public'],
  core: { disableTelemetry: true },
  async viteFinal(cfg) {
    const { default: react } = await import('@vitejs/plugin-react');
    // Tailwind for the shadcn components (config in src/styles.css: tw: prefix, no preflight).
    const { default: tailwindcss } = await import('@tailwindcss/vite');
    cfg.plugins = [...(cfg.plugins ?? []), react(), tailwindcss()];
    return cfg;
  },
};

export default config;
