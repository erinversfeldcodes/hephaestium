// What a page must do, written once and used for each page, theme and width. Each check throws
// with what it found, so that a failure says what is wrong and not only that something is.
import AxeBuilder from "@axe-core/playwright";
import type { Page } from "@playwright/test";

/** The controls a person can reach and use: the browser's own list of what takes focus, minus what is hidden or disabled. */
const NATIVE = "a[href], button:not([disabled]), input:not([type=hidden]):not([disabled]), select:not([disabled]), textarea:not([disabled]), summary";

const CONTROLS = [
  "a[href]",
  "button:not([disabled])",
  "input:not([type=hidden]):not([disabled])",
  "select:not([disabled])",
  "textarea:not([disabled])",
  "summary",
  "[tabindex]",
].join(",");

interface Reached {
  readonly name: string;
  readonly outlineStyle: string;
  readonly outlineWidth: number;
}

async function expectedControls(page: Page): Promise<string[]> {
  return page.evaluate(([selector, NATIVE]) => {
    const label = (el: Element) =>
      `${el.tagName.toLowerCase()} ${(el.getAttribute("aria-label") ?? el.textContent ?? "").trim().replace(/\s+/g, " ").slice(0, 40)}`;
    return [...document.querySelectorAll(selector)]
      // A control the page took out of the Tab order is still a control, so it is expected and not reached; only an
      // element that is no control but a place the page put focus on by hand is left out when it is out of the order.
      .filter((el) => el.matches(NATIVE) || (el as HTMLElement).tabIndex >= 0)
      .filter((el) => el.getClientRects().length > 0 && getComputedStyle(el).visibility !== "hidden")
      .map(label);
  }, [CONTROLS, NATIVE] as const);
}

async function reached(page: Page): Promise<Reached | null> {
  return page.evaluate(() => {
    const el = document.activeElement;
    if (el === null || el === document.body || el === document.documentElement) return null;
    const style = getComputedStyle(el);
    return {
      name: `${el.tagName.toLowerCase()} ${(el.getAttribute("aria-label") ?? el.textContent ?? "").trim().replace(/\s+/g, " ").slice(0, 40)}`,
      outlineStyle: style.outlineStyle,
      outlineWidth: parseFloat(style.outlineWidth),
    };
  });
}

/** Tab walks every control in the order the page lays them out, never gets stuck, and shows where it is. */
export async function checkKeyboard(page: Page): Promise<string[]> {
  const expected = await expectedControls(page);
  if (expected.length === 0) throw new Error("the page has no control to reach by keyboard");

  await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
  const visited: string[] = [];
  const bare: string[] = [];
  // One more press than there are controls: the last one leaves the page or wraps to the first.
  for (let press = 0; press <= expected.length + 2; press += 1) {
    await page.keyboard.press("Tab");
    const now = await reached(page);
    if (now === null) break;
    if (visited.length > 0 && now.name === visited[0] && visited.length >= expected.length) break;
    visited.push(now.name);
    if (now.outlineStyle === "none" || !(now.outlineWidth > 0)) bare.push(now.name);
  }

  // A scrolling region the browser makes focusable is not a control the page declared: leave it out of the order.
  const inOrder = visited.filter((name) => expected.includes(name));
  if (JSON.stringify(inOrder) !== JSON.stringify(expected)) {
    throw new Error(`Tab did not reach the controls in page order.\n expected: ${expected.join(" | ")}\n reached:  ${visited.join(" | ")}`);
  }
  if (bare.length > 0) throw new Error(`no visible focus indicator on: ${bare.join(" | ")}`);
  return expected;
}

/** The page is as wide as the window: nothing makes it scroll sideways. */
export async function checkNoSideScroll(page: Page): Promise<void> {
  const found = await page.evaluate(() => {
    const viewport = document.documentElement.clientWidth;
    // Content that spills past the window is clipped, not scrolled, where the page hides its overflow; so the
    // reach of every box is read as well as the document's width. A table that scrolls inside its own box is held in it.
    const spilling = [...document.body.querySelectorAll("*")]
      .filter((el) => !el.closest(".table-scroll") && el.getClientRects().length > 0)
      .filter((el) => el.getBoundingClientRect().right > viewport + 1)
      .map((el) => `${el.tagName.toLowerCase()}${el.className ? "." + String(el.className) : ""}`);
    return {
      viewport,
      document: document.documentElement.scrollWidth,
      body: document.body.scrollWidth,
      spilling,
    };
  });
  if (found.document !== found.viewport || found.body > found.viewport || found.spilling.length > 0) {
    throw new Error(`the page is wider than the window: ${JSON.stringify(found)}`);
  }
}

/** No serious or critical accessibility violation, which includes the contrast of text in the theme on show. */
export async function checkAccessible(page: Page): Promise<void> {
  const result = await new AxeBuilder({ page }).analyze();
  const serious = result.violations.filter((v) => v.impact === "serious" || v.impact === "critical");
  if (serious.length > 0) {
    const lines = serious.map((v) => `${v.id} (${v.impact}): ${v.nodes.map((n) => n.target.join(" ")).join("; ")}`);
    throw new Error(`accessibility violations:\n${lines.join("\n")}`);
  }
}

/** Counts the clicks a page receives, across the pages it moves to. Whatever the keyboard activates is a click on the control. */
export async function watchClicks(page: Page): Promise<() => number> {
  let clicks = 0;
  await page.exposeFunction("__reportClick", () => {
    clicks += 1;
  });
  await page.addInitScript(() => {
    document.addEventListener("click", () => void (window as unknown as { __reportClick: () => void }).__reportClick(), true);
  });
  return () => clicks;
}

/**
 * Each control does what its kind does from the keyboard: a link follows on Enter, a button acts on
 * Enter and on Space, a field takes typing. Each is tried on a fresh page, since acting may change it.
 */
export async function checkActivation(page: Page, clicks: () => number, open: () => Promise<void>, quiet: () => Promise<void>): Promise<void> {
  await open();
  const controls = await expectedControls(page);
  const reopen = async () => {
    await open();
    const again = await expectedControls(page);
    if (JSON.stringify(again) !== JSON.stringify(controls)) {
      throw new Error(`the page offered other controls when opened again.\n first: ${controls.join(" | ")}\n again: ${again.join(" | ")}`);
    }
  };
  for (let index = 0; index < controls.length; index += 1) {
    await reopen();
    const kind = await focusControl(page, index);
    if (kind === "region") continue;
    if (kind === "field") {
      await page.keyboard.type("x");
      const typed = await page.evaluate(() => (document.activeElement as HTMLInputElement).value);
      if (!typed.includes("x")) throw new Error(`control ${index} is a field that took no typing`);
      continue;
    }
    const keys = kind === "link" ? ["Enter"] : ["Enter", "Space"];
    for (const key of keys) {
      if (key !== keys[0]) {
        await reopen();
        await focusControl(page, index);
      }
      const before = clicks();
      const where = page.url();
      await page.keyboard.press(key);
      // A press is answered in milliseconds (the slowest whole test took 9.7s for dozens of presses),
      // so 2s is a wide margin; past it the control is reported as not activated.
      const deadline = Date.now() + 2000;
      while (clicks() === before && Date.now() < deadline) await page.waitForTimeout(25);
      if (clicks() === before) throw new Error(`control ${index} (${kind}) was not activated by ${key}`);
      // A link that navigates does so within a frame or two; 500ms is the longest wait, and a link
      // that goes nowhere only costs that. A link may be taking the page elsewhere: let that finish before the page is opened again.
      if (kind === "link") await page.waitForURL((url) => url.href !== where, { timeout: 500 }).catch(() => undefined);
      // What the control sent is answered before the page is opened again, or the answer lands on the next page.
      await quiet();
    }
  }
}

async function focusControl(page: Page, index: number): Promise<"link" | "button" | "field" | "region"> {
  await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
  for (let press = 0; press <= index; press += 1) await page.keyboard.press("Tab");
  return page.evaluate(() => {
    const el = document.activeElement as HTMLElement;
    if (el.tagName === "A") return "link";
    if (el.tagName === "BUTTON") return "button";
    // A scrolling region takes the arrow keys, which the browser provides; it has nothing to activate.
    return el.tagName === "DIV" || el.tagName === "LI" ? "region" : "field";
  });
}

/** The theme asked for is the one on show: the page's background is light or dark as the theme says. */
export async function checkTheme(page: Page, shows: "light" | "dark"): Promise<void> {
  const lightness = await page.evaluate(() => {
    const [r = 0, g = 0, b = 0] = getComputedStyle(document.body).backgroundColor.match(/[\d.]+/g)?.map(Number) ?? [];
    return (0.2126 * r + 0.7152 * g + 0.0722 * b) / 255;
  });
  const seen = lightness > 0.5 ? "light" : "dark";
  if (seen !== shows) throw new Error(`the ${shows} theme was asked for and the page shows the ${seen} one`);
}
