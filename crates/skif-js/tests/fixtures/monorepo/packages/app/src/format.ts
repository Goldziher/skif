export function format(...args: unknown[]): string {
  return args.join(",");
}

export const unusedFormat = 1;

/** @public */
export const apiFormat = 2;

export type FormatOptions = { pad: number };
