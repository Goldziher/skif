import { expect, test, vi } from "vitest";
import { a } from "./a";

vi.mock("./gone", () => ({ b: 2 }));
test("a", () => expect(a).toBe(1));
