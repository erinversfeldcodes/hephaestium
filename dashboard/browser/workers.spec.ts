import { expect, test, type Page } from "@playwright/test";
import { origin, prepare, sizes, themes } from "./pages";

// Presses Tab until the control with this accessible name has focus: the keyboard only, in page order.
async function tabTo(page: Page, name: string): Promise<void> {
  await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
  for (let press = 0; press < 40; press += 1) {
    await page.keyboard.press("Tab");
    const label = await page.evaluate(
      () => document.activeElement?.getAttribute("aria-label") ?? document.activeElement?.textContent?.trim() ?? "",
    );
    if (label === name) return;
  }
  throw new Error(`Tab never reached "${name}"`);
}

const blocks = ["Device token", "Save command", "Command"] as const;

for (const size of sizes) {
  test.describe(`the token flow at ${size.name}px`, () => {
    test.use({ viewport: { width: size.width, height: size.height } });

    test.describe("with the clipboard allowed", () => {
      test.use({ permissions: ["clipboard-read", "clipboard-write"] });

      test("a token is made and each block is copied, by keyboard alone", async ({ page }) => {
        const shown = await prepare(page, origin, themes[0]);
        await shown.open("/workers", "laptop");

        await tabTo(page, "");
        await page.keyboard.type("build box");
        await page.keyboard.press("Enter");
        await expect(page.getByRole("heading", { name: 'Device token "build box"' })).toBeVisible();

        for (const block of blocks) {
          await tabTo(page, `Copy ${block.toLowerCase()}`);
          await page.keyboard.press("Space");
          await expect(page.getByRole("button", { name: `Copy ${block.toLowerCase()}` })).toHaveText("Copied");

          const shownText = await page.getByLabel(block, { exact: true }).innerText();
          const copied = await page.evaluate(() => navigator.clipboard.readText());
          expect(copied).toBe(shownText);
        }

        // What was copied is what makes a worker run: the secret, in a file named for the token.
        await expect(page.getByLabel("Device token", { exact: true })).toHaveText("heph_secret_once");
        await expect(page.getByLabel("Save command", { exact: true })).toContainText("heph_secret_once");
        await expect(page.getByLabel("Save command", { exact: true })).toContainText("worker-token-build-box");
        await expect(page.getByLabel("Command", { exact: true })).toContainText(`--control-plane ${origin}`);
        expect(shown.answers.unanswered).toEqual([]);
      });
    });

    test.describe("with the clipboard refused", () => {
      test("the text is selected, and the page says to copy it by hand", async ({ page }) => {
        // The browser lets a click or key write to the clipboard, so a refusal is made here, as the browser makes it.
        await page.addInitScript(() => {
          Object.defineProperty(navigator, "clipboard", {
            value: { writeText: () => Promise.reject(new DOMException("blocked", "NotAllowedError")) },
          });
        });
        const shown = await prepare(page, origin, themes[0]);
        await shown.open("/workers", "laptop");

        await tabTo(page, "");
        await page.keyboard.type("build box");
        await page.keyboard.press("Enter");
        await expect(page.getByRole("heading", { name: 'Device token "build box"' })).toBeVisible();

        for (const block of blocks) {
          await tabTo(page, `Copy ${block.toLowerCase()}`);
          await page.keyboard.press("Enter");
          const copyable = page.locator(".copyable", { has: page.getByLabel(block, { exact: true }) });
          await expect(copyable.getByRole("status")).toContainText("copy it by hand");
          const selected = await page.evaluate(() => window.getSelection()?.toString() ?? "");
          expect(selected).toBe(await page.getByLabel(block, { exact: true }).innerText());
          await expect(page.getByRole("button", { name: `Copy ${block.toLowerCase()}` })).toHaveText("Copy");
        }
      });
    });
  });
}
