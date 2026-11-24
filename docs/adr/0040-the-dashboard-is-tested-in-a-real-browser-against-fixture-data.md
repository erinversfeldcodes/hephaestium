# 0040. The dashboard is tested in a real browser against fixture data

## Status

Accepted

## Context

The dashboard's tests run in jsdom. jsdom has no layout, no real focus order and no colour scheme, so no test can show that a page is reached by keyboard, that it reads in the light and the dark theme, or that nothing scrolls sideways at 390px. Tickets that require those things could only be checked by a person looking, so the completeness judge marked the requirement owed and the ticket stayed open.

Running a browser is a new technology in this project. The test AGENTS.md sets is whether a principled engineer would build this here even if no one were watching. What the dashboard is required to do is a property of a rendered page, and nothing but a rendered page can show it.

## Decision

- **Playwright, driving Chromium, runs the built dashboard** (`vite preview` of `npm run build`), as the control plane serves it. Chromium only: the dashboard is for the people who run this project, and a second engine is a cost with no requirement behind it.
- **The API is answered by intercepting requests**, from the fixtures the dashboard's own tests already use. Nothing else runs: no control plane, database or login. The fixtures hold what the client reads, so one module writes them as the API sends them, and a unit test proves the real client reads that back as the fixtures.
- **Each page is shown in four themes and at two widths.** The themes are the browser's light and dark preference, and the page's own `data-theme` setting under the opposite preference, which shows the setting wins. The widths are 1280px and 390px.
- **Three things are checked on each**: that Tab reaches every control in page order, is never trapped and shows a focus indicator, and that each control works from the keyboard as its kind does; that the page is as wide as the window; and that axe finds no serious or critical violation, which includes text contrast in the theme on show.
- **A check is trusted only once it has refused a planted fault**: no focus indicator, a control out of the Tab order, an element wider than the window, text that fails contrast in dark. These are tests of their own.
- **Screenshots of every page at every theme and width are kept as an artifact** of the CI run for a person to look at. They are not compared with anything: a saved-snapshot comparison is a separate decision.
- **The browser tests run in their own CI job.** It is reported and does not block until it has passed ten runs in a row, then it joins the `ci` gate. A test that fails once is retried once, and the retry is reported, so a flaky test is seen. Browsers are cached between runs.
- **A requirement about keyboard, theme or layout is written "a browser test shows ...".** The completeness judge can then settle it on a named test.

## Consequences

- Playwright and axe are test-only dependencies of the dashboard, and CI downloads Chromium (cached).
- The browser tests are not in the pre-push gates: they run in CI first.
- jsdom tests stay for what they are good at, which is logic and markup; they are not asked to show what they cannot.
