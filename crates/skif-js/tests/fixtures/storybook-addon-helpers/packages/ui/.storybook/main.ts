import type { StorybookConfig } from '@storybook/vue3-vite';
import { dirname } from 'path';
import { fileURLToPath } from 'url';

function getAbsolutePath(value: string): string {
  return dirname(fileURLToPath(import.meta.resolve(`${value}/package.json`)));
}

const config: StorybookConfig = {
  stories: ['../src/**/*.stories.ts'],
  addons: [getAbsolutePath('@storybook/addon-a11y'), { name: getAbsolutePath('@storybook/addon-docs') }],
  framework: { name: getAbsolutePath('@storybook/vue3-vite'), options: {} },
};
export default config;
