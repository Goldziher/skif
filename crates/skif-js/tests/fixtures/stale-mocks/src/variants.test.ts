import { expect, test, vi } from "vitest";
import "./missing-import";

vi.doMock("./gone-do-mock");
vi.unmock("./gone-unmock");
vi.mock("./icon.svg?react");
vi.mock("./missing-icon.svg?react");
vi.mock("@/lib/real");
vi.mock("@/lib/gone");
vi.mock("./helper");
vi.mock("not-installed-package");

test("variants", async () => {
  expect(await vi.importActual("./gone-actual")).toBeDefined();
});
vi.mock(import("./gone-typed"), () => ({}));
