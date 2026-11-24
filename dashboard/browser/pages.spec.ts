import { expect, test } from "@playwright/test";
import { checkAccessible, checkActivation, checkKeyboard, checkNoSideScroll, checkTheme, watchClicks } from "./checks";
import { origin, pages, prepare, sizes, themes } from "./pages";

// Every page, in each theme and at each width. The screenshots are kept for a person to look at;
// they are not compared with anything.
for (const route of pages) {
  for (const theme of themes) {
    for (const size of sizes) {
      test.describe(`${route.name}, ${theme.name}, ${size.name}px`, () => {
        test.use({ viewport: { width: size.width, height: size.height } });

        test("is reached by Tab in order, with a visible focus", async ({ page }) => {
          const shown = await prepare(page, origin, theme);
          await shown.open(route.path, route.ready);
          await checkKeyboard(page);
          expect(shown.answers.unanswered).toEqual([]);
        });

        test("each control is used from the keyboard as its kind is", async ({ page }) => {
          const shown = await prepare(page, origin, theme);
          const clicks = await watchClicks(page);
          await checkActivation(page, clicks, () => shown.open(route.path, route.ready), () => shown.answers.quiet());
        });

        test("does not scroll sideways, shows the theme, and has no serious accessibility violation", async ({ page }) => {
          const shown = await prepare(page, origin, theme);
          await shown.open(route.path, route.ready);
          await page.screenshot({ path: `browser-screenshots/${route.name}-${theme.name}-${size.name}.png`, fullPage: true });
          await checkTheme(page, theme.shows);
          await checkNoSideScroll(page);
          await checkAccessible(page);
        });
      });
    }
  }
}
