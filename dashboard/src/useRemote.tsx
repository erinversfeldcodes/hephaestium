import { useEffect, useState, type ReactNode } from "react";
import { ApiError } from "./api";

export type Remote<T> =
  | { readonly kind: "loading" }
  | { readonly kind: "loaded"; readonly value: T }
  | { readonly kind: "failed"; readonly error: unknown };

/** Runs a read when the page opens and again when what it depends on changes. */
export function useRemote<T>(load: () => Promise<T>, dependencies: readonly unknown[]): Remote<T> {
  const [state, setState] = useState<Remote<T>>({ kind: "loading" });

  useEffect(() => {
    let current = true;
    setState({ kind: "loading" });
    load().then(
      (value) => {
        if (current) setState({ kind: "loaded", value });
      },
      (error: unknown) => {
        if (current) setState({ kind: "failed", error });
      },
    );
    return () => {
      current = false;
    };
  }, dependencies);

  return state;
}

/** What a read looks like while it is not yet a value: waiting, failed, or signed out. */
export function Shown<T>({
  state,
  children,
}: {
  readonly state: Remote<T>;
  readonly children: (value: T) => ReactNode;
}) {
  if (state.kind === "loading") return <p>Loading</p>;
  if (state.kind === "loaded") return <>{children(state.value)}</>;
  if (state.error instanceof ApiError && state.error.status === 401) {
    return (
      <p>
        Your session has ended. <a href="/auth/login">Sign in with GitHub</a>
      </p>
    );
  }
  return <p role="alert">This page could not be read from the control plane.</p>;
}
