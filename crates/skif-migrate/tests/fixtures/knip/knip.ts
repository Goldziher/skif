import type { KnipConfig } from 'knip';

const config: KnipConfig = {
  entry: ['src/index.ts!', 'scripts/*.ts'],
  project: ['src/**/*.ts!', 'scripts/**/*.ts'],
  ignore: ['src/generated/**'],
  ignoreFiles: ['src/legacy/**'],
  ignoreDependencies: ['left-pad', /^@demo\/.+$/, '^eslint-', 'tsx|ts-node'],
  ignoreBinaries: ['docker', 'lefthook'],
  ignoreUnresolved: ['#internal/(.*)', '\\.svg$'],
  ignoreMembers: ['render', /^handle/],
  ignoreExportsUsedInFile: true,
  ignoreWorkspaces: ['examples/**'],
  includeEntryExports: true,
  ignoreIssues: { 'src/api/**': ['exports', 'types'] },
  rules: { files: 'warn', unlisted: 'off', exports: 'error' },
  tags: ['-lintignore', '+public'],
  treatConfigHintsAsErrors: true,
  vitest: { config: ['vitest.config.ts', 'vitest.workspace.ts'], entry: ['**/*.test.ts'] },
  storybook: false,
  compilers: { mdx: (text: string) => text },
  paths: { '@/*': ['./src/*'] },
  workspaces: {
    '.': { entry: ['src/index.ts'] },
    'apps/web': {
      entry: ['src/main.tsx', 'src/routes/**/*.tsx!'],
      project: ['src/**/*.{ts,tsx}!'],
      ignoreDependencies: ['@demo/*', /^virtual:/],
      ignoreUnresolved: ['virtual:routes'],
      paths: { '~/*': ['./src/*'] },
      includeEntryExports: true,
      vitest: 'vitest.web.config.ts',
      playwright: { config: 'e2e/playwright.config.ts', entry: ['e2e/**/*.spec.ts'] },
      ignoreIssues: { 'src/legacy/**': ['files'] },
    },
    'packages/*': {
      ignoreBinaries: [computeBinaries()],
      entry: ['src/index.ts', ...extraEntries],
      mystery: true,
    },
  },
};

export default config;
