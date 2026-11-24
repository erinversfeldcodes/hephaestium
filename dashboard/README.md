# Dashboard

A single-page app the control plane serves. `npm run dev` runs it against a control plane at `HEPH_DASHBOARD_API` (default `http://localhost:4000`).

| Command | What it checks |
| --- | --- |
| `npm run typecheck` | Types, including the browser tests |
| `npm test` | Logic and markup, in jsdom (vitest) |
| `npm run test:browser` | Keyboard, theme and layout, in Chromium (Playwright) |

## Browser tests

They build the dashboard, serve it, and answer the control plane's API from `src/fixtures.ts` by intercepting requests, so no control plane, database or login is needed. For each page (board, ticket, run, spend, workers), in the light and dark theme (by the browser's preference and by the page's own `data-theme`) and at 1280px and 390px, they check that Tab reaches every control in order with a visible focus, that each control works from the keyboard, that nothing scrolls sideways, and that axe finds no serious or critical violation. The Workers page's token flow is also run by keyboard alone, with the clipboard allowed and refused. `browser/planted.spec.ts` shows each check refusing a planted fault.

First time only: `npx playwright install chromium`. Then `npm run test:browser`; add `-g "workers, dark-setting, 390px"` to run one. A screenshot of each page, theme and width is written to `browser-screenshots/`, and CI keeps them as the `dashboard-screenshots` artifact. A failed test leaves a trace in `browser-results/`, opened with `npx playwright show-trace`.

When you write a ticket, write a requirement about keyboard, a theme or a layout as "a browser test shows ...", so that it can be settled on a named test and not on someone looking. See [the decision](../docs/adr/0040-the-dashboard-is-tested-in-a-real-browser-against-fixture-data.md).
