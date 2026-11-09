jest.mock("./gone-jest");
jest.doMock("./gone-jest-do-mock", () => ({}));
jest.mock("./virtual-module", () => ({}), { virtual: true });
const actual = jest.requireActual("./gone-required");

export const used = actual;
