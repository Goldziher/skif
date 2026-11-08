import { a1, bb } from "./chain";
import { Color } from "./color";
import { x, y } from "./dupe";
import * as ns from "./namespace";
import type { Shape } from "./shape";
import "./side";

export const main = (shape: Shape): number => a1 + bb + ns.alpha + Color.Red + x + y + shape.sides;

export const lazy = () => import("./lazy");

export const plugins = import.meta.glob("./plugins/*.ts");

// Loaded at runtime by name; the compiled `.js` file is the `.ts` source.
export const reporter = (name: string) => import(`./reporters/${name}.js`);
