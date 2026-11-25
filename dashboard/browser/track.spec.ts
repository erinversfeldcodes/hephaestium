import { expect, test } from "@playwright/test";
import { origin, prepare, sizes, themes } from "./pages";

// A segment of a row's track names its step and says its state, on hover and on focus, inside the screen, in each theme.
for (const theme of themes) {
  for (const size of sizes) {
    test.describe(`the board's track, ${theme.name}, ${size.name}px`, () => {
      test.use({ viewport: { width: size.width, height: size.height } });

      for (const how of ["hover", "focus"] as const) {
        test(`a segment shows its label on ${how}, within the screen and readable`, async ({ page }) => {
          const shown = await prepare(page, origin, theme);
          await shown.open("/board", "Board");
          for (const index of [0, 5]) {
            const segment = page.locator(".track").first().locator("li").nth(index);
            if (how === "hover") await segment.hover();
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
            expect(box.color).not.toBe(box.background);
            expect(box.height).toBeGreaterThan(0);
            expect(box.left).toBeGreaterThanOrEqual(0);
            expect(box.right).toBeLessThanOrEqual(box.viewport);
          }
        });
      }
    });
  }
}
