import produce from "immer";
import { nanoid } from "nanoid";
import { assertNever, groupBy } from "./utils";
import type { Item } from "./store";

export type Action =
  | { type: "added"; title: string; tags?: string[] }
  | { type: "removed"; id: string }
  | { type: "renamed"; id: string; title: string }
  | { type: "tagged"; id: string; tag: string }
  | { type: "cleared" };

export interface TodoState {
  items: Record<string, Item>;
  order: string[];
}

export const initial: TodoState = { items: {}, order: [] };

export const reducer = produce((draft: TodoState, action: Action) => {
  switch (action.type) {
    case "added": {
      const id = nanoid();
      draft.items[id] = { id, title: action.title, tags: action.tags ?? [], createdAt: new Date() };
      draft.order.push(id);
      break;
    }
    case "removed":
      delete draft.items[action.id];
      draft.order = draft.order.filter((id) => id !== action.id);
      break;
    case "renamed":
      if (draft.items[action.id]) draft.items[action.id].title = action.title;
      break;
    case "tagged": {
      const item = draft.items[action.id];
      if (item && !item.tags.includes(action.tag)) item.tags.push(action.tag);
      break;
    }
    case "cleared":
      return initial;
    default:
      assertNever(action);
  }
}, initial);

export const selectByTag = (state: TodoState): Record<string, Item[]> =>
  groupBy(
    state.order.map((id) => state.items[id]).filter((item): item is Item => item !== undefined),
    (item) => item.tags[0] ?? "untagged",
  );

export const selectCount = (state: TodoState) => state.order.length;
