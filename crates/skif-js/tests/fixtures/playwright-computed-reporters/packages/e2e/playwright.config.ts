import { defineConfig } from '@playwright/test';

import { sharedReporters } from './shared-config';

export default defineConfig({
  testDir: './tests',
  reporter: [
    ...sharedReporters(),
    ['./reporters/local-reporter.ts'],
    ...(process.env.A11Y ? [['./reporters/a11y-reporter.ts']] : []),
  ],
});
