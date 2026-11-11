import isexe from "isexe";
import isNumber from "is-number";

export const check = (value) => isNumber(value) && isexe;
