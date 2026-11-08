import { defineConfig } from 'vitest/config';

import { setupFiles } from './setup.js';

export const createVitestConfig = (options = {}) =>
	defineConfig({
		test: {
			include: ['src/**/*.check.ts'],
			setupFiles,
			...options,
		},
	});

export const vitestConfig = createVitestConfig();
