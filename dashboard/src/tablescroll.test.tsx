import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { fakeClient } from "./fixtures";
import { RunPage } from "./views/RunPage";
import { SpendPage } from "./views/SpendPage";
import { TicketDetailPage } from "./views/TicketDetailPage";
import { TicketListPage } from "./views/TicketListPage";
import { WorkersPage } from "./views/WorkersPage";

const never = () => new Promise<never>(() => undefined);

/** Every table on the page, each of which must sit in a container that scrolls sideways. */
function expectScrolling(container: HTMLElement, tables: number) {
  const all = Array.from(container.querySelectorAll("table"));
  expect(all).toHaveLength(tables);
  for (const table of all) {
    const scroller = table.closest(".table-scroll");
    expect(scroller).not.toBeNull();
    // Without a place in the Tab order the keyboard cannot scroll it.
    expect(scroller).toHaveAttribute("tabindex", "0");
  }
}

describe("every table is inside a scrolling container", () => {
  it("on the ticket list", async () => {
    const { container } = render(<TicketListPage client={fakeClient()} />);
    await screen.findByRole("table");
    expectScrolling(container, 1);
  });

  it("on the ticket page: its runs and its findings by lens", async () => {
    const { container } = render(
      <TicketDetailPage client={fakeClient()} repository="erinversfeldcodes/gazebolang" number={12} />,
    );
    await screen.findByRole("table", { name: "Findings by lens" });
    expectScrolling(container, 2);
  });

  it("on a run's page: its gates", async () => {
    const { container } = render(<RunPage client={fakeClient()} id="w-2" follow={never} />);
    await screen.findByRole("table", { name: "Gates" });
    expectScrolling(container, 1);
  });

  it("on the spend page", async () => {
    const { container } = render(<SpendPage client={fakeClient()} repository="erinversfeldcodes/gazebolang" />);
    await screen.findByRole("table");
    expectScrolling(container, 1);
  });

  it("on the workers page", async () => {
    const { container } = render(<WorkersPage client={fakeClient()} />);
    await screen.findByRole("table");
    expectScrolling(container, 1);
  });
});
