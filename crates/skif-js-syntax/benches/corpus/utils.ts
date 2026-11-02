export function describe(error: unknown): string {
  if (error instanceof Error) return `${error.name}: ${error.message}`;
  return typeof error === "string" ? error : JSON.stringify(error);
}

export const clamp = (value: number, min: number, max: number): number => Math.min(Math.max(value, min), max);

export function groupBy<T, K extends PropertyKey>(values: Iterable<T>, key: (value: T) => K): Record<K, T[]> {
  const result = {} as Record<K, T[]>;
  for (const value of values) {
    (result[key(value)] ??= []).push(value);
  }
  return result;
}

export function debounce<A extends unknown[]>(fn: (...args: A) => void, wait: number) {
  let timer: ReturnType<typeof setTimeout> | undefined;
  return (...args: A): void => {
    if (timer !== undefined) clearTimeout(timer);
    timer = setTimeout(() => fn(...args), wait);
  };
}

export function* chunk<T>(values: readonly T[], size: number): Generator<T[], void, undefined> {
  for (let i = 0; i < values.length; i += size) {
    yield values.slice(i, i + size);
  }
}

export type DeepPartial<T> = { [K in keyof T]?: T[K] extends object ? DeepPartial<T[K]> : T[K] };
export type Awaited2<T> = T extends Promise<infer U> ? Awaited2<U> : T;
export type Keys<T> = T extends unknown ? keyof T : never;

export function assertNever(value: never): never {
  throw new Error(`unexpected value: ${String(value)}`);
}

export const isDefined = <T>(value: T | null | undefined): value is T => value != null;

export namespace Path {
  export const sep = "/";
  export function join(...parts: string[]): string {
    return parts
      .filter(Boolean)
      .join(sep)
      .replace(/\/{2,}/g, sep);
  }
  export function basename(path: string): string {
    return path.slice(path.lastIndexOf(sep) + 1);
  }
}

export default { describe, clamp, groupBy, debounce, chunk, isDefined };
