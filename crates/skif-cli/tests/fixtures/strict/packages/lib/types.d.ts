import { Declared as Base } from 'declared-types';
import { Runtime } from 'declared-dep';
import { Typed } from 'typed-only';

export type Declared = Base | Runtime | Typed;
