import type { ReactNode } from "react";

/**
 * A table keeps its text whole and scrolls sideways inside this container, never the page. The
 * container takes focus, so that someone who uses only the keyboard can scroll it.
 */
export function TableScroll({ children }: { readonly children: ReactNode }) {
  return (
    <div className="table-scroll" tabIndex={0}>
      {children}
    </div>
  );
}
