jest.mock("./generated", () => ({}), { virtual: true });
jest.mock("virtual-package", () => ({}), { virtual: true });
const actual = jest.requireActual("@scope/not-installed");

export const used = actual;
