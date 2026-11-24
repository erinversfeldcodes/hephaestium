import type { Page } from "@playwright/test";
import { answerFromFixtures, type Answers } from "./server";

export const origin = "http://127.0.0.1:4173";

export const pages = [
  { name: "board", path: "/board", ready: "Board" },
  { name: "ticket", path: "/tickets/erinversfeldcodes/gazebolang/12", ready: "Teach the parser about guards" },
  { name: "run", path: "/runs/w-2", ready: "Finished" },
  { name: "spend", path: "/spend?repository=erinversfeldcodes%2Fgazebolang", ready: "Spend" },
  { name: "workers", path: "/workers", ready: "Workers" },
] as const;

/**
 * How the theme is chosen. The browser's preference is what a page follows when it is not told;
 * the page's own setting is the `data-theme` it carries, which wins over the preference, so each
 * setting is shown under the opposite preference.
 */
export const themes = [
  { name: "light-preference", scheme: "light", setting: null, shows: "light" },
  { name: "dark-preference", scheme: "dark", setting: null, shows: "dark" },
  { name: "light-setting", scheme: "dark", setting: "light", shows: "light" },
  { name: "dark-setting", scheme: "light", setting: "dark", shows: "dark" },
] as const;

export const sizes = [
  { name: "1280", width: 1280, height: 900 },
  { name: "390", width: 390, height: 844 },
] as const;

export type Theme = (typeof themes)[number];

export interface Opened {
  readonly answers: Answers;
  /** Loads the address and waits until the page has what it shows. */
  open(path: string, ready: string): Promise<void>;
}

/** Sets the page up to be shown from the fixtures, in a theme. */
export async function prepare(page: Page, origin: string, theme: Theme): Promise<Opened> {
  const answers = await answerFromFixtures(page, origin);
  await page.emulateMedia({ colorScheme: theme.scheme, reducedMotion: "reduce" });
  if (theme.setting !== null) {
    const setting = theme.setting;
    // The script runs before the document has its root, so the attribute is set once the document is parsed.
    await page.addInitScript((value) => {
      const set = () => document.documentElement.setAttribute("data-theme", value);
      if (document.documentElement) set();
      document.addEventListener("DOMContentLoaded", set);
    }, setting);
  }
  return {
    answers,
    async open(path, ready) {
      await page.goto(path);
      await page.getByText(ready, { exact: false }).first().waitFor();
      // Controls arrive with the data, so the page is ready once nothing on it is still loading.
      await page.getByText("Loading", { exact: true }).waitFor({ state: "detached" });
      await page.waitForLoadState("networkidle");
    },
  };
}
