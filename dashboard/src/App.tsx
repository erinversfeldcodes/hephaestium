import { useEffect, useState } from "react";
import type { Client, Session } from "./api";

type State =
  | { readonly kind: "loading" }
  | { readonly kind: "signed-out" }
  | { readonly kind: "signed-in"; readonly session: Session }
  | { readonly kind: "unreachable" };

export function App({ client }: { readonly client: Client }) {
  const [state, setState] = useState<State>({ kind: "loading" });

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

  return (
    <main>
      <h1>Hephaestium</h1>
      {state.kind === "loading" && <p>Loading</p>}
      {state.kind === "signed-out" && <a href="/auth/login">Sign in with GitHub</a>}
      {state.kind === "signed-in" && (
        <p>
          <span>{state.session.login}</span> <a href="/auth/logout">Sign out</a>
        </p>
      )}
      {state.kind === "unreachable" && <p role="alert">The control plane could not be reached.</p>}
    </main>
  );
}
