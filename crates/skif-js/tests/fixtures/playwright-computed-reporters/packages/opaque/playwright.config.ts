import { defineConfig } from '@playwright/test';

const reportersFromEnv = () => JSON.parse(process.env.PW_REPORTERS ?? '[]');

export default defineConfig({
  testDir: './tests',
  reporter: [...reportersFromEnv(), ['./reporters/local-reporter.ts']],
});
