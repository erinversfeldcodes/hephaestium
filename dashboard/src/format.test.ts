import { describe, expect, it } from "vitest";
import { amount, amounts } from "./format";

describe("amounts of spend", () => {
  it("writes dollars as money and tokens as a count, each on its own", () => {
    expect(amount("usd", 1.5)).toBe("$1.50");
    expect(amount("tokens", 12345)).toBe("12,345 tokens");
    expect(amount("credits", 3)).toBe("3 credits");
  });

  it("never adds one unit to another, and says so when nothing was spent", () => {
    expect(amounts({ usd: 1.5, tokens: 12345 })).toBe("$1.50 and 12,345 tokens");
    expect(amounts({})).toBe("nothing");
  });
});
