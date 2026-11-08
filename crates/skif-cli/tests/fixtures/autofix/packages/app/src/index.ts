import isNumber from "is-number";
import { Color, Size, Space } from "./color";
import { name } from "./defaults";

console.log(isNumber(1), Color.Red, Size.Large, Space.used, name);
