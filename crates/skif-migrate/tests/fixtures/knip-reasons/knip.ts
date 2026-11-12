import type { KnipConfig } from 'knip';

const config: KnipConfig = {
  // installed on the CI image, not in package.json
  ignoreBinaries: ['docker', 'make'],
  ignoreDependencies: [
    'unexplained',
    // loaded by the bundler from a config string
    'some-loader',
    'other-loader',
    'left-pad', // kept for the legacy build
    'after-trailing',
    // types only, pulled in by codegen
    '@demo/types',
    '@demo/runtime',
  ],
  ignoreUnresolved: [ // virtual modules of the bundler
    'virtual:routes',
    /^virtual:/,
  ],
  workspaces: {
    'packages/app': {
      // resolved by the host page at run time
      ignoreDependencies: [
        'host-sdk',
        // the one exception: loaded from a CDN
        'cdn-widget',
      ],
    },
  },
};

export default config;
