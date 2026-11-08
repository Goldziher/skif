import run from 'runtime-dep';
import type { Shape } from 'type-dep';
import type { Dev } from 'dev-types';
import devRuntime from 'dev-runtime';
import peer from 'used-peer';
import optional from 'optional-peer';
import type { Declared } from './types';

export type { Other } from 'type-dep';

export const value: Shape | Dev | Declared = run(devRuntime, peer, optional);
