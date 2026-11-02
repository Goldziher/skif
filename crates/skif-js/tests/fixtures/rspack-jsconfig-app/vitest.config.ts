import { defineConfig } from 'vitest/config';

export default defineConfig({ test: { include: ['spec-src/**/*.check.js'] } });
