import { defineConfig } from '@playwright/test';

export default defineConfig({
  testDir: './tests',
  reporter: [['list'], ['./reporters/local-reporter.ts', { outputFile: process.env.REPORT_FILE }]],
});
