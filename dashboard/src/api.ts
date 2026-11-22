// The dashboard reads the control plane's API and writes nothing: every request below is a GET,
// and `get` is the only function that touches the network, so there is no way to send another
// method from here. A person's session is the cookie the browser already holds.

export class ApiError extends Error {
  readonly status: number;

  constructor(status: number, message: string) {
    super(message);
    this.name = "ApiError";
    this.status = status;
  }
}

export interface Session {
  readonly login: string;
}

export interface Client {
  /** Who is signed in, or null when nobody is. Any other failure raises. */
  session(): Promise<Session | null>;
}

type Fetcher = typeof fetch;

export function createClient(fetcher: Fetcher = (...args) => fetch(...args)): Client {
  async function get(path: string): Promise<Response> {
    return fetcher(path, { method: "GET", credentials: "same-origin" });
  }

  return {
    async session() {
      const response = await get("/api/session");
      if (response.status === 401) return null;
      if (!response.ok) throw new ApiError(response.status, response.statusText);
      return parseSession(await response.json());
    },
  };
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function parseSession(body: unknown): Session {
  if (isRecord(body) && typeof body["login"] === "string") return { login: body["login"] };
  throw new ApiError(200, "the session is not in the shape the API promises");
}
