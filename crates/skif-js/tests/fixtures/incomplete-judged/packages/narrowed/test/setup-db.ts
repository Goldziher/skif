import { seed } from '../src/seed';

seed();

// The file is in the narrowed scope and may be an entry the shared config names: its exports are not judged.
export const setupHelper = 1;
