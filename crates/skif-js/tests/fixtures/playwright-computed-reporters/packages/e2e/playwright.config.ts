import { defineConfig } from '@playwright/test';

import { sharedReporters } from './shared-config';

export default defineConfig({
  testDir: './tests',
  reporter: [...sharedReporters(), ['./reporters/local-reporter.ts']],
});
