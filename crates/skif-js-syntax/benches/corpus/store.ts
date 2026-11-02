import { EventEmitter } from "node:events";
import type { Logger } from "./logger";
import { createLogger } from "./logger";
import * as utils from "./utils";
export { utils };
export type { Logger } from "./logger";

export interface State {
  readonly items: ReadonlyMap<string, Item>;
  readonly revision: number;
}

export interface Item {
  id: string;
  title: string;
  tags: string[];
  createdAt: Date;
  meta?: Record<string, unknown>;
}

export enum Status {
  Idle = "idle",
  Loading = "loading",
  Failed = "failed",
}

export const enum Limits {
  MaxItems = 1000,
  MaxTags = 16,
}

type Listener<T> = (value: T, previous: T) => void;
type Selector<T, R> = (state: T) => R;

export class Store<T extends State = State> extends EventEmitter {
  private current: T;
  private readonly listeners = new Set<Listener<T>>();
  public status: Status = Status.Idle;
  static #instances = 0;
  #log: Logger;

  constructor(initial: T, logger: Logger = createLogger("store")) {
    super();
    this.current = initial;
    this.#log = logger;
    Store.#instances++;
  }

  get state(): T {
    return this.current;
  }

  subscribe(listener: Listener<T>): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  select<R>(selector: Selector<T, R>): R {
    return selector(this.current);
  }

  update(patch: Partial<T>): void {
    const previous = this.current;
    this.current = { ...previous, ...patch, revision: previous.revision + 1 };
    this.#log.debug(`revision ${this.current.revision}`);
    for (const listener of this.listeners) {
      listener(this.current, previous);
    }
    this.emit("change", this.current);
  }

  async load(fetcher: () => Promise<Item[]>): Promise<void> {
    this.status = Status.Loading;
    try {
      const items = await fetcher();
      const map = new Map(items.slice(0, Limits.MaxItems).map((item) => [item.id, item] as const));
      this.update({ items: map } as unknown as Partial<T>);
      this.status = Status.Idle;
    } catch (error) {
      this.status = Status.Failed;
      this.#log.error(utils.describe(error));
      throw error;
    }
  }

  static get count(): number {
    return Store.#instances;
  }
}

export function createStore(items: Item[] = []): Store {
  return new Store({ items: new Map(items.map((i) => [i.id, i])), revision: 0 });
}

export default Store;
