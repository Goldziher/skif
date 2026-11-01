import { greet } from "@fx/lib";
import { shout } from "@fx/lib/util";
import debounce from "lodash/debounce";
import { join } from "node:path";
import React from "react";
import chalk from "chalk";

import { format } from "@/format";
import { missing } from "./missing";
import type { Options } from "./options";

export const options: Options = { loud: true };

export const run = (): string => format(shout(greet(join("a", "b"))), debounce, React, chalk, missing);
