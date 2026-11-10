import { mergeConfig } from 'vitest/config';

import { shared } from './vitest.base';

// The shared config is unknown, but it can only name tests and setup modules: the scope is narrowed.
export default mergeConfig(shared, { test: { environment: 'node' } });
