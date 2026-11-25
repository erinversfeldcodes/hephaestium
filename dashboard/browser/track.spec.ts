import { expect, test } from "@playwright/test";
import { origin, prepare, sizes, themes } from "./pages";

/** The contrast ratio of two colours as the browser reports them, rgb() or color(srgb ...). */
export function contrast(a: string, b: string): number {
  const luminance = (css: string): number => {
    const numbers = css.match(/[\d.]+/g)!.map(Number);
    const unit = css.startsWith("color(") ? numbers.slice(-3) : numbers.slice(0, 3).map((n) => n / 255);
    const [r, g, bl] = unit.map((c) => (c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4)) as [number, number, number];
    return 0.2126 * r + 0.7152 * g + 0.0722 * bl;
  };
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x) as [number, number];
  return (hi + 0.05) / (lo + 0.05);
}

// A segment of a row's track names its step and says its state, on hover, on focus and on tap, inside the screen, in each theme.
for (const theme of themes) {
  for (const size of sizes) {
    test.describe(`the board's track, ${theme.name}, ${size.name}px`, () => {
      test.use({ viewport: { width: size.width, height: size.height }, hasTouch: true });

      for (const how of ["hover", "focus", "tap"] as const) {
        test(`a segment shows its label on ${how}, within the screen and readable`, async ({ page }) => {
          const shown = await prepare(page, origin, theme);
          await shown.open("/board", "Board");
          for (const index of [0, 5]) {
            const segment = page.locator(".track").first().locator("li").nth(index);
            if (how === "hover") await segment.hover();
            else if (how === "tap") await segment.tap();
            else await segment.focus();
            const label = (await segment.getAttribute("data-say"))!;
            expect(label).toMatch(/^(Run|Gates|Review|CI|Merged|Judged): /);
            const box = await segment.evaluate((el) => {
              const after = getComputedStyle(el, "::after");
              const track = el.parentElement!.getBoundingClientRect();
              return {
                content: after.content,
                color: after.color,
                background: after.backgroundColor,
                // The browser's used values for the label's own box, which a pseudo-element cannot report as a rectangle.
                left: track.left + parseFloat(after.left),
                right: track.left + parseFloat(after.left) + parseFloat(after.width),
                height: parseFloat(after.height),
                viewport: document.documentElement.clientWidth,
              };
            });
            expect(box.content).toBe(`"${label}"`);
            expect(contrast(box.color, box.background)).toBeGreaterThanOrEqual(4.5);
            expect(box.height).toBeGreaterThan(0);
            expect(box.left).toBeGreaterThanOrEqual(0);
            expect(box.right).toBeLessThanOrEqual(box.viewport);
          }
        });
      }
    });
  }
}

test("a label whose colour is close to its background fails the contrast check", () => {
  expect(contrast("rgb(20, 20, 20)", "rgb(255, 255, 255)")).toBeGreaterThanOrEqual(4.5);
  expect(contrast("rgb(200, 200, 200)", "rgb(255, 255, 255)")).toBeLessThan(4.5);
});
