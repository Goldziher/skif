import { a } from "./a";
import type { C } from "./c";

export const main = (value: C): number => a + value.d;
