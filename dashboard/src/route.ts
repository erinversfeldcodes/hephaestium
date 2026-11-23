export type Route =
  | { readonly page: "tickets" }
  | { readonly page: "board" }
  | { readonly page: "run"; readonly id: string }
  | { readonly page: "ticket"; readonly repository: string; readonly number: number }
  | { readonly page: "spend"; readonly repository: string | null }
  | { readonly page: "workers" }
  | { readonly page: "not-found" };

/** The page an address names. */
export function parseRoute(pathname: string, search: string): Route {
  const segments = pathname.split("/").filter((segment) => segment !== "");

  if (segments.length === 0) return { page: "tickets" };

  if (segments[0] === "tickets" && segments.length === 4) {
    const [, owner, name, raw] = segments;
    const number = Number(raw);
    if (owner && name && /^[0-9]+$/.test(raw ?? "") && Number.isSafeInteger(number) && number > 0) {
      return { page: "ticket", repository: `${owner}/${name}`, number };
    }
  }

  if (segments[0] === "board" && segments.length === 1) return { page: "board" };

  if (segments[0] === "workers" && segments.length === 1) return { page: "workers" };

  if (segments[0] === "runs" && segments.length === 2 && /^[A-Za-z0-9_-]+$/.test(segments[1] ?? "")) {
    return { page: "run", id: segments[1] ?? "" };
  }

  if (segments[0] === "spend" && segments.length === 1) {
    return { page: "spend", repository: new URLSearchParams(search).get("repository") };
  }

  return { page: "not-found" };
}

export function ticketPath(repository: string, number: number): string {
  return `/tickets/${repository}/${number}`;
}

export function spendPath(repository: string | null): string {
  return repository === null ? "/spend" : `/spend?repository=${encodeURIComponent(repository)}`;
}

export function runPath(id: string): string {
  return `/runs/${id}`;
}
