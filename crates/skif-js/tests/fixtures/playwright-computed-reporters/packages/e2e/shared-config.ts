import type { ReporterDescription } from '@playwright/test';
import { resolve } from 'node:path';

export function sharedReporters(): ReporterDescription[] {
  const shared: ReporterDescription[] = [[resolve(__dirname, 'reporters/metrics-reporter.ts')]];

  if (!process.env.CI) {
    return [['html'], ...shared, ['list']];
  }

  return [['list'], ...shared];
}
