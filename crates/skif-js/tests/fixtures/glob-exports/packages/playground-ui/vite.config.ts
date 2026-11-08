import { readdirSync } from 'node:fs';
import { defineConfig } from 'vite';

// One library entry per component directory, published through the `./components/*` export.
function componentEntries() {
  return Object.fromEntries(
    readdirSync('src/ds/components').map((name) => [`components/${name}`, `src/ds/components/${name}/index.ts`]),
  );
}

export default defineConfig({
  build: {
    lib: {
      entry: { index: 'src/index.ts', ...componentEntries() },
      formats: ['es'],
    },
  },
});
