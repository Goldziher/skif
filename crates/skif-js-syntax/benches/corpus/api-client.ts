import { z } from "zod";
import { setTimeout as sleep } from "node:timers/promises";
import type { RequestInit, Response } from "undici";

const ItemSchema = z.object({
  id: z.string().uuid(),
  title: z.string().min(1).max(120),
  tags: z.array(z.string()).default([]),
});

export type ItemDto = z.infer<typeof ItemSchema>;

export class HttpError extends Error {
  constructor(
    public readonly status: number,
    message: string,
    public readonly body?: unknown,
  ) {
    super(message);
    this.name = "HttpError";
  }
}

interface ClientOptions {
  baseUrl: string;
  retries?: number;
  timeoutMs?: number;
  headers?: Record<string, string>;
}

const defaults = { retries: 3, timeoutMs: 5_000 } as const;

export function createClient(options: ClientOptions) {
  const { baseUrl, retries = defaults.retries, timeoutMs = defaults.timeoutMs, headers = {} } = options;

  async function request<T>(path: string, init: RequestInit = {}, schema?: z.ZodType<T>): Promise<T> {
    let attempt = 0;
    for (;;) {
      const controller = new AbortController();
      const timer = globalThis.setTimeout(() => controller.abort(), timeoutMs);
      try {
        const response = (await fetch(new URL(path, baseUrl), {
          ...init,
          headers: { accept: "application/json", ...headers, ...(init.headers as Record<string, string>) },
          signal: controller.signal,
        } as never)) as unknown as Response;
        if (!response.ok) {
          throw new HttpError(response.status, `${init.method ?? "GET"} ${path} failed`, await response.text());
        }
        const payload: unknown = await response.json();
        return schema ? schema.parse(payload) : (payload as T);
      } catch (error) {
        if (error instanceof HttpError && error.status < 500) throw error;
        if (++attempt > retries) throw error;
        await sleep(2 ** attempt * 100);
      } finally {
        clearTimeout(timer);
      }
    }
  }

  return {
    listItems: () => request("/items", {}, z.array(ItemSchema)),
    getItem: (id: string) => request(`/items/${encodeURIComponent(id)}`, {}, ItemSchema),
    createItem: (item: Omit<ItemDto, "id">) =>
      request("/items", { method: "POST", body: JSON.stringify(item) } as RequestInit, ItemSchema),
    removeItem: async (id: string): Promise<void> => {
      await request<void>(`/items/${id}`, { method: "DELETE" } as RequestInit);
    },
  };
}

export type Client = ReturnType<typeof createClient>;
