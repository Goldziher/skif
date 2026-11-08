import { defineConfig, mergeConfig } from 'vite';
import { vitestConfig } from '@fx/vitest-config/frontend';

export default mergeConfig(defineConfig({ build: { outDir: 'dist' } }), vitestConfig);
