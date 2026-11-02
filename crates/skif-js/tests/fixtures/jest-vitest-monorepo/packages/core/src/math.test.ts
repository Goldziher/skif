import { add, sub } from './math';

test('math', () => {
  expect(add(1, 2)).toBe(3);
  expect(sub(3, 2)).toBe(1);
});
