import { useRef, useState, type FormEvent } from "react";
import type { Client, DeviceToken, IssuedToken } from "../api";
import { when } from "../format";
import { Shown, useRemote } from "../useRemote";

const CONFIG_DIR = "~/.config/hephaestium";

// Where `heph worker` looks for a token when it is given none. It is never offered for a new
// token: a second token saved there would replace the first worker's.
export const DEFAULT_TOKEN_FILE = `${CONFIG_DIR}/worker-token`;

// The file a token is saved to: named after the token, so that one token never overwrites another.
export function tokenFile(name: string): string {
  const safe = name
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
  return `${CONFIG_DIR}/worker-token-${safe === "" ? "device" : safe}`;
}

function shellQuote(text: string): string {
  return `'${text.replace(/'/g, `'\\''`)}'`;
}

// A block of text to copy, with a button beside it. Where the browser refuses the clipboard the
// text is selected, so that it can be copied by hand.
function Copyable({ label, text }: { readonly label: string; readonly text: string }) {
  const block = useRef<HTMLPreElement>(null);
  const [state, setState] = useState<"idle" | "copied" | "manual">("idle");

  async function copy() {
    try {
      await navigator.clipboard.writeText(text);
      setState("copied");
      setTimeout(() => setState("idle"), 2000);
    } catch {
      const node = block.current;
      if (node !== null) {
        const range = document.createRange();
        range.selectNodeContents(node);
        const selection = window.getSelection();
        selection?.removeAllRanges();
        selection?.addRange(range);
      }
      setState("manual");
    }
  }

  return (
    <div className="copyable">
      <pre ref={block} aria-label={label}>
        {text}
      </pre>
      <button type="button" onClick={() => void copy()} aria-label={`Copy ${label.toLowerCase()}`}>
        {state === "copied" ? "Copied" : "Copy"}
      </button>
      {state === "manual" && <span role="status">Your browser did not allow copying. The text is selected: copy it by hand.</span>}
    </div>
  );
}

export function WorkersPage({
  client,
  origin = window.location.origin,
}: {
  readonly client: Client;
  readonly origin?: string;
}) {
  const [version, setVersion] = useState(0);
  // The secret lives in this state only: a reload, or leaving the page, loses it for good.
  const [issued, setIssued] = useState<IssuedToken | null>(null);
  const [failure, setFailure] = useState<string | null>(null);
  const [name, setName] = useState("");
  const file = issued === null ? "" : tokenFile(issued.token.name);
  const tokens = useRemote(() => client.deviceTokens(), [client, version]);

  async function make(event: FormEvent) {
    event.preventDefault();
    setFailure(null);
    const result = await client.makeDeviceToken(name.trim());
    if (result.ok) {
      setIssued(result.value);
      setName("");
      setVersion((v) => v + 1);
    } else {
      setFailure(result.error);
    }
  }

  async function revoke(token: DeviceToken) {
    setFailure(null);
    const result = await client.revokeDeviceToken(token.id);
    if (!result.ok) setFailure(result.error);
    if (issued?.token.id === token.id) setIssued(null);
    setVersion((v) => v + 1);
  }

  return (
    <section aria-labelledby="workers-page-heading">
      <h2 id="workers-page-heading">Workers</h2>
      {failure !== null && <p role="alert">{failure}</p>}

      {issued !== null && (
        <section aria-labelledby="new-token-heading">
          <h3 id="new-token-heading">{`Device token "${issued.token.name}"`}</h3>
          <p>This is the only time the token is shown. Copy it now.</p>
          <Copyable label="Device token" text={issued.secret} />
          <p>{`Save it to ${file}, which only you can read:`}</p>
          <Copyable label="Save command" text={`umask 077; printf '%s' ${shellQuote(issued.secret)} > ${file}`} />
          <p>Then run the worker, naming that file:</p>
          <Copyable label="Command" text={`HEPH_WORKER_TOKEN_FILE=${file} heph worker --control-plane ${origin}`} />
          <p>{`To use this token as this machine's only worker, you may instead save it as ${DEFAULT_TOKEN_FILE}, which heph worker reads when no file is named.`}</p>
          <button type="button" onClick={() => setIssued(null)}>
            I have saved it
          </button>
        </section>
      )}

      <form onSubmit={make}>
        <label>
          Name the worker, for example the machine it runs on
          <input value={name} maxLength={80} onChange={(event) => setName(event.target.value)} />
        </label>{" "}
        <button type="submit" disabled={name.trim() === ""} aria-describedby="make-reason">
          Make a device token
        </button>
        {name.trim() === "" && <span id="make-reason"> Type a name first: a token needs one.</span>}
      </form>

      <Shown state={tokens}>
        {(rows) =>
          rows.length === 0 ? (
            <p>You have no device tokens.</p>
          ) : (
            <div className="table-scroll">
<table>
              <thead>
                <tr>
                  <th>Name</th>
                  <th>Created</th>
                  <th>Last connected</th>
                  <th>Connected now</th>
                  <th />
                </tr>
              </thead>
              <tbody>
                {rows.map((row) => (
                  <tr key={row.id}>
                    <td>{row.name}</td>
                    <td>{when(row.createdAt)}</td>
                    <td>{when(row.lastSeenAt)}</td>
                    <td>{row.revokedAt !== null ? "revoked" : row.connected ? "yes" : "no"}</td>
                    <td>
                      {row.revokedAt === null && (
                        <button type="button" onClick={() => void revoke(row)}>
                          {`Revoke ${row.name}`}
                        </button>
                      )}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
</div>
          )
        }
      </Shown>
    </section>
  );
}
