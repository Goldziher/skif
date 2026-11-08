import text from "./text/plain.ts?raw";
import workletUrl from "./worklet.ts?url";

// Modules: a plain glob, and queries that still load the module.
export const pages = import.meta.glob("./pages/*.ts", { eager: true });
export const inline = import.meta.glob("./styles/*.ts", { query: "?inline" });
export const flagged = import.meta.glob("./flags/*.ts", { query: { raw: true } });
// Source text only: the matches are not loaded as modules.
export const sources = import.meta.glob("./components/*.ts", { query: "?raw", import: "default", eager: true });
export const legacy = import.meta.glob("./legacy/*.ts", { as: "raw" });
export const objects = import.meta.glob("./objects/*.ts", { query: { raw: "" } });

export { text, workletUrl };
