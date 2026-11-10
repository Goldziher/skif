// Built at run time: its value is not known statically.
export const shared = JSON.parse(process.env.VITEST_BASE ?? '{}');
