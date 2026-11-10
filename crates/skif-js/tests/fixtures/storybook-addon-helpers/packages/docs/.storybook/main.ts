// The addon is chosen at run time: which package it names is not known statically.
export default {
  stories: ['../src/**/*.stories.ts'],
  addons: [process.env.STORYBOOK_ADDON],
  framework: '@storybook/react-vite',
};
