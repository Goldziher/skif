import { expect, it, vi } from "vitest";
import "left-pad";
import { run } from "./index";

vi.mock("axios");
vi.mock("framer-motion", () => ({ motion: {} }));
vi.mock("./helper");

const fixture = `
  vi.mock("inv1");
  jest.mock('inv2');
`;
const inline = "jest.requireActual('inv3')";

it("runs", () => {
  expect([run(), fixture, inline]).toBeDefined();
});
