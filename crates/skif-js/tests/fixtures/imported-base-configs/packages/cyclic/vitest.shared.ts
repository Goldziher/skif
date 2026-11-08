import { mergeConfig } from 'vitest/config';

import config from './vitest.config';

// Imports the config that imports it: the cycle is not followed, so the config is not fully known.
export const shared = mergeConfig(config, { test: { include: ['src/**/*.check.ts'] } });
