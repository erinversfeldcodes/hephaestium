import { expect, test } from "@playwright/test";
import { checkAccessible, checkKeyboard, checkNoSideScroll } from "./checks";
import { origin, prepare, themes } from "./pages";

// A check that has never failed is not known to work. Each is shown a page with one fault put in,
// and must refuse it, for the reason it exists.
test.describe("a planted fault is refused", () => {
  test.use({ viewport: { width: 390, height: 844 } });

  test("a page with no focus indicator", async ({ page }) => {
    const shown = await prepare(page, origin, themes[0]);
    await shown.open("/board", "Board");
    await checkKeyboard(page);
    await page.addStyleTag({ content: "*:focus, *:focus-visible { outline: none !important; }" });
    await expect(checkKeyboard(page)).rejects.toThrow(/no visible focus indicator/);
  });

  test("a control that Tab does not reach", async ({ page }) => {
    const shown = await prepare(page, origin, themes[0]);
    await shown.open("/board", "Board");
    await page.evaluate(() => document.querySelector("main a[href]")?.setAttribute("tabindex", "-1"));
    await expect(checkKeyboard(page)).rejects.toThrow(/did not reach the controls/);
  });

  test("an element wider than the window", async ({ page }) => {
    const shown = await prepare(page, origin, themes[0]);
    await shown.open("/board", "Board");
    await checkNoSideScroll(page);
    await page.evaluate(() => {
      const box = document.createElement("div");
      box.style.width = "600px";
      box.textContent = "fixed";
      document.querySelector("main")?.append(box);
    });
    await expect(checkNoSideScroll(page)).rejects.toThrow(/wider than the window/);
  });

  test("text whose colour fails contrast in the dark theme", async ({ page }) => {
    const shown = await prepare(page, origin, themes[1]);
    await shown.open("/workers", "Workers");
    await checkAccessible(page);
    await page.addStyleTag({ content: "main h2, main label { color: #555 !important; }" });
    await expect(checkAccessible(page)).rejects.toThrow(/color-contrast/);
  });
});
