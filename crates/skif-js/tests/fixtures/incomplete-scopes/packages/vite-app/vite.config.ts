import { mergeConfig } from 'vite';

import base from './vite.base';

// The base is unknown; a shared base can only name the conventional `src/main.ts`.
export default mergeConfig(base, { server: { port: 3000 } });
