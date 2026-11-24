import { useState, type FormEvent } from "react";
import type { Client, DeviceToken, IssuedToken } from "../api";
import { when } from "../format";
import { Shown, useRemote } from "../useRemote";

// Where `heph worker` looks for a token when it is given none.
export const TOKEN_FILE = "~/.config/hephaestium/worker-token";

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
          <pre aria-label="Device token">{issued.secret}</pre>
          <p>{`Save it to ${TOKEN_FILE} with mode 600, then run:`}</p>
          <pre aria-label="Command">{`heph worker --control-plane ${origin}`}</pre>
          <button type="button" onClick={() => setIssued(null)}>
            I have saved it
          </button>
        </section>
      )}

      <form onSubmit={make}>
        <label>
          Name
          <input value={name} maxLength={80} onChange={(event) => setName(event.target.value)} />
        </label>{" "}
        <button type="submit" disabled={name.trim() === ""}>
          Make a device token
        </button>
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
                  <th>Running</th>
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
                    <td>{row.revokedAt !== null ? "-" : `${row.running} of ${row.slots}`}</td>
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
