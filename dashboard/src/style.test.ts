import { describe, expect, it } from "vitest";
import stylesheet from "./style.css?raw";

const tokenNames = [
  "bg", "surface", "raised", "line", "fg", "muted", "accent", "accent-text", "accent-soft",
  "success", "success-soft", "error", "error-soft", "warning", "warning-soft",
];

// The colours any rule may hold are the tokens, so the one block declaring them is cut out first.
function outsideTokens(css: string): string {
  const start = css.indexOf(":root {");
  const end = css.indexOf("}", start);
  return css.slice(0, start) + css.slice(end + 1);
}

const namedColours =
  /(?<![-\w])(white|black|red|green|blue|gray|grey|orange|yellow|purple|pink|brown|silver|navy|teal|cyan|magenta|lime|maroon|olive|aqua|fuchsia)(?![-\w])/i;

/** The first hard-coded colour in the CSS text, or null. */
export function hardCodedColour(css: string): string | null {
  const stripped = css.replace(/\/\*[\s\S]*?\*\//g, "");
  const match = stripped.match(/#[0-9a-f]{3,8}\b|\b(?:rgba?|hsla?|hwb|lab|lch|oklab|oklch|color)\(/i) ?? stripped.match(namedColours);
  return match ? match[0] : null;
}

describe("the stylesheet", () => {
  it("holds no colour outside the token block", () => {
    expect(hardCodedColour(outsideTokens(stylesheet))).toBeNull();
  });

  it("the check fails on a planted hard-coded colour", () => {
    expect(hardCodedColour(outsideTokens(`${stylesheet}\na { color: #ff0000; }`))).toBe("#ff0000");
    expect(hardCodedColour(outsideTokens(`${stylesheet}\na { color: rgb(0 0 0); }`))).toBe("rgb(");
    expect(hardCodedColour(outsideTokens(`${stylesheet}\na { color: red; }`))).toBe("red");
  });

  it.each(tokenNames)("token --%s has a light and a dark value", (name) => {
    expect(stylesheet).toMatch(new RegExp(`--${name}:\\s*light-dark\\(\\s*#[0-9a-f]{6}\\s*,\\s*#[0-9a-f]{6}\\s*\\)\\s*;`, "i"));
  });

  it("supports both schemes and an explicit theme override", () => {
    expect(stylesheet).toContain("color-scheme: light dark");
    expect(stylesheet).toContain('[data-theme="light"]');
    expect(stylesheet).toContain('[data-theme="dark"]');
  });

  it("has a phone breakpoint at 40rem", () => {
    expect(stylesheet).toMatch(/@media \(max-width: 40rem\)/);
  });

  // The declarations of the first rule whose selector is exactly this one, or null.
  function declarations(css: string, selector: string): string | null {
    const stripped = css.replace(/\/\*[\s\S]*?\*\//g, "");
    const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
    return stripped.match(new RegExp(`(?:^|[}\\s])${escaped}\\s*\\{([^}]*)\\}`))?.[1] ?? null;
  }

  it("scrolls a table inside its container, and keeps cells and pills whole", () => {
    expect(declarations(stylesheet, ".table-scroll")).toMatch(/overflow-x:\s*auto/);
    expect(declarations(stylesheet, ".table-scroll th,\n.table-scroll td")).toMatch(/white-space:\s*nowrap/);
    expect(declarations(stylesheet, ".state")).toMatch(/white-space:\s*nowrap/);
  });

  it("the table check fails on a planted violation", () => {
    const planted = stylesheet.replace(/(\.table-scroll \{[^}]*?)overflow-x:\s*auto;/, "$1");
    expect(planted).not.toBe(stylesheet);
    expect(declarations(planted, ".table-scroll")).not.toMatch(/overflow-x:\s*auto/);
  });

  it("sets row titles in the foreground colour, with the accent on hover", () => {
    expect(declarations(stylesheet, ".title > a")).toMatch(/color:\s*var\(--fg\)/);
    expect(declarations(stylesheet, ".title > a:hover")).toMatch(/color:\s*var\(--accent-text\)/);
  });

  it("sets table links in the foreground colour, with the accent on hover", () => {
    expect(declarations(stylesheet, "td a")).toMatch(/color:\s*var\(--fg\)/);
    expect(declarations(stylesheet, "td a:hover")).toMatch(/color:\s*var\(--accent-text\)/);
  });
});
