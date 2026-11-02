import { describe, expect, it } from 'vitest';

import { helper } from '../tests/helpers';
import { total } from './main';

describe('main', () => {
  it('adds', () => {
    expect(total).toBe(helper('3'));
  });
});
