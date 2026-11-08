import { mergeConfig } from 'vitest/config';

import { shared } from './vitest.base';

// The shared config may name setup files: test directories and setup modules are entries.
export default mergeConfig(shared, { test: { environment: 'node' } });
