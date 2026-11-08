import * as constants from './constants';

// A bare `typeof import` names the module's type and reads none of its exports.
export type Constants = typeof import('./constants');
export type Mocked = typeof import('./mocked');

// A qualified one reads the export it names, and the member after it.
export type Red = typeof import('./enums').Color.Red;
export type AnySize = typeof import('./enums').Size;

console.log(constants.USED);
