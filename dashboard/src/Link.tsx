import { createContext, useContext, type MouseEvent, type ReactNode } from "react";

type Navigate = (path: string) => void;

// Outside the app a link is an ordinary one: the browser loads the address.
export const NavigateContext = createContext<Navigate>((path) => window.location.assign(path));

/** A link to a page of the dashboard that changes the page without loading it again. */
export function Link({ to, children, current }: { readonly to: string; readonly children: ReactNode; readonly current?: boolean }) {
  const navigate = useContext(NavigateContext);

  function follow(event: MouseEvent<HTMLAnchorElement>) {
    if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
    event.preventDefault();
    navigate(to);
  }

  return (
    <a href={to} onClick={follow} aria-current={current ? "page" : undefined}>
      {children}
    </a>
  );
}
