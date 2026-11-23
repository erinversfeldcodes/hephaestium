import { useCallback, useEffect, useState } from "react";
import type { Client, Session } from "./api";
import { Link, NavigateContext } from "./Link";
import { parseRoute, spendPath, type Route } from "./route";
import { BoardPage } from "./views/BoardPage";
import { SpendPage } from "./views/SpendPage";
import { TicketDetailPage } from "./views/TicketDetailPage";
import { TicketListPage } from "./views/TicketListPage";

type State =
  | { readonly kind: "loading" }
  | { readonly kind: "signed-out" }
  | { readonly kind: "signed-in"; readonly session: Session }
  | { readonly kind: "unreachable" };

function currentRoute(): Route {
  return parseRoute(window.location.pathname, window.location.search);
}

export function App({ client }: { readonly client: Client }) {
  const [state, setState] = useState<State>({ kind: "loading" });
  const [route, setRoute] = useState<Route>(currentRoute);

  useEffect(() => {
    let current = true;
    client
      .session()
      .then((session) => {
        if (current) setState(session ? { kind: "signed-in", session } : { kind: "signed-out" });
      })
      .catch(() => {
        if (current) setState({ kind: "unreachable" });
      });
    return () => {
      current = false;
    };
  }, [client]);

  // The back and forward buttons change the address without the app asking.
  useEffect(() => {
    const onPop = () => setRoute(currentRoute());
    window.addEventListener("popstate", onPop);
    return () => window.removeEventListener("popstate", onPop);
  }, []);

  const navigate = useCallback((path: string) => {
    window.history.pushState({}, "", path);
    setRoute(currentRoute());
  }, []);

  return (
    <NavigateContext.Provider value={navigate}>
      <header>
        <h1>Hephaestium</h1>
        {state.kind === "signed-in" && (
          <nav aria-label="Pages">
            <Link to="/board">Board</Link> <Link to="/">Tickets</Link> <Link to={spendPath(null)}>Spend</Link>
          </nav>
        )}
        {state.kind === "signed-in" && (
          <p>
            <span>{state.session.login}</span> <a href="/auth/logout">Sign out</a>
          </p>
        )}
      </header>
      <main>
        {state.kind === "loading" && <p>Loading</p>}
        {state.kind === "signed-out" && <a href="/auth/login">Sign in with GitHub</a>}
        {state.kind === "unreachable" && (
          <p role="alert">The control plane could not be reached.</p>
        )}
        {state.kind === "signed-in" && <Page client={client} route={route} />}
      </main>
    </NavigateContext.Provider>
  );
}

function Page({ client, route }: { readonly client: Client; readonly route: Route }) {
  switch (route.page) {
    case "tickets":
      return <TicketListPage client={client} />;
    case "board":
      return <BoardPage client={client} />;
    case "ticket":
      return <TicketDetailPage client={client} repository={route.repository} number={route.number} />;
    case "spend":
      return <SpendPage client={client} repository={route.repository} />;
    case "not-found":
      return <p>There is no such page.</p>;
  }
}
