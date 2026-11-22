import { useState } from "react";
import type { Client, Question, ReplyResult } from "../api";
import { when } from "../format";
import { useRemote } from "../useRemote";

/**
 * The question a run ended with, if one is waiting, and a box to answer it as the person who
 * is signed in. The question is shown as text, never as markup. A question that cannot be read
 * leaves the rest of the page as it is.
 */
export function QuestionPanel({
  client,
  repository,
  number,
}: {
  readonly client: Client;
  readonly repository: string;
  readonly number: number;
}) {
  const asked = useRemote(() => client.question(repository, number), [client, repository, number]);

  if (asked.kind !== "loaded" || asked.value === null) return null;
  return <Answering client={client} question={asked.value} />;
}

type Sending =
  | { readonly kind: "writing" }
  | { readonly kind: "sending" }
  | { readonly kind: "refused"; readonly result: ReplyResult }
  | { readonly kind: "sent" };

function Answering({ client, question }: { readonly client: Client; readonly question: Question }) {
  const [text, setText] = useState("");
  const [state, setState] = useState<Sending>({ kind: "writing" });

  async function send() {
    if (state.kind === "sending" || text.trim() === "") return;
    setState({ kind: "sending" });
    let result: ReplyResult;
    try {
      result = await client.reply(question.repository, question.ticket, text.trim());
    } catch {
      result = { commented: false, approved: false, error: "The control plane could not be reached, and nothing was changed." };
    }
    setState(result.error === null ? { kind: "sent" } : { kind: "refused", result });
  }

  return (
    <section aria-labelledby="question-heading">
      <h3 id="question-heading">Question</h3>
      <p>Asked {when(question.askedAt)}.</p>
      <blockquote style={{ whiteSpace: "pre-wrap" }}>{question.question}</blockquote>

      {state.kind === "sent" ? (
        <p role="status">Your answer was posted on the ticket, and you approved it again as yourself.</p>
      ) : (
        <form
          onSubmit={(event) => {
            event.preventDefault();
            void send();
          }}
        >
          <label>
            Your answer
            <textarea
              value={text}
              onChange={(event) => setText(event.target.value)}
              rows={5}
              maxLength={8000}
              disabled={state.kind === "sending"}
            />
          </label>
          <button type="submit" disabled={state.kind === "sending" || text.trim() === ""}>
            {state.kind === "sending" ? "Sending" : "Send answer"}
          </button>
          {state.kind === "refused" && (
            <p role="alert">
              {state.result.error}
              {state.result.commented && !state.result.approved
                ? " Your comment was posted but the ticket is not approved."
                : ""}
            </p>
          )}
        </form>
      )}
    </section>
  );
}
