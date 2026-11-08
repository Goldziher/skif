import isNumber from "is-number";
import { Color, Size, Space } from "./color";
import { name } from "./defaults";
import { pluginName } from "./plugin";

console.log(isNumber(1), Color.Red, Size.Large, Space.used, name, pluginName);
