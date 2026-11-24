//! The executor's side of the link to a control plane: four requests, each opened from this
//! machine, each carrying the device token and nothing else that is secret.
//!
//! Nothing here keeps a connection that the control plane could use to reach back, and nothing
//! decides what a failure means for a run: it says which of a few things happened, and the
//! worker decides what to do.

use std::fmt;
use std::time::Duration;

use serde_json::Value;

/// How long any request may take. A little over the control plane's long poll, which it ends
/// at about 25 seconds, so a poll that is only waiting is never cut here.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// The most of a control plane's answer that is repeated in a message.
const SHOWN: usize = 200;

/// A device token. It is never shown: not by `Debug`, and not in any message made here.
#[derive(Clone)]
pub struct Token(String);

impl Token {
    #[must_use]
    pub fn new(value: String) -> Self {
        Self(value)
    }

    /// Whether `text` holds the token.
    #[must_use]
    pub fn is_in(&self, text: &str) -> bool {
        !self.0.is_empty() && text.contains(&self.0)
    }
}

impl fmt::Debug for Token {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Token(..)")
    }
}

/// The value of the `Authorization` header, which holds the token and is never shown.
#[derive(Clone)]
struct Bearer(String);

impl fmt::Debug for Bearer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Bearer(..)")
    }
}

/// What a request that did not succeed came to.
#[derive(Debug, PartialEq, Eq)]
pub enum Failure {
    /// The control plane does not hold this workload for this token: the lease lapsed, was let
    /// go, or was never this token's. Whatever the executor is doing for it is to stop.
    Lost,
    /// The control plane does not accept the token. Asking again will not change that.
    Denied,
    /// The control plane refused this request for good (a rule of the stream broken, a body it
    /// cannot read). Sending it again will not change that.
    Refused(String),
    /// The request did not get an answer that settles anything: the connection failed or was
    /// cut, it took too long, or the control plane had trouble. It is sent again.
    Trouble(String),
}

impl fmt::Display for Failure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Lost => formatter.write_str("the control plane no longer holds this workload"),
            Self::Denied => formatter.write_str("the control plane does not accept the token"),
            Self::Refused(why) | Self::Trouble(why) => formatter.write_str(why),
        }
    }
}

/// A workload the control plane gave this machine.
#[derive(Debug)]
pub struct Claim {
    pub workload: Value,
    pub id: String,
    pub lease: Duration,
    /// The number the first event of this run takes.
    pub next_sequence: u64,
}

/// A connection to one control plane.
#[derive(Clone, Debug)]
pub struct Client {
    agent: ureq::Agent,
    base: String,
    token: Token,
    bearer: Bearer,
}

impl Client {
    /// A client for the control plane at `address`, with the token and no other credential.
    ///
    /// # Errors
    ///
    /// Returns why the address cannot be used: it is not an http or https address, it holds
    /// something besides a host and a path, or it is plain http to a host that is not this
    /// machine, which would send the token in the clear.
    pub fn new(address: &str, token: Token) -> Result<Self, String> {
        let base = address.trim().trim_end_matches('/').to_owned();
        let (scheme, rest) = base
            .split_once("://")
            .ok_or_else(|| format!("{address} is not an address: it needs http:// or https://"))?;
        let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
        if host.is_empty() || host.contains('@') {
            return Err(format!(
                "{address} is not an address of the form https://host[:port]"
            ));
        }
        match scheme {
            "https" => {}
            "http" if is_this_machine(host) => {}
            "http" => {
                return Err(format!(
                    "{address} is plain http to another machine, which would send the token in the clear: use https"
                ));
            }
            other => return Err(format!("{address}: {other} is not http or https")),
        }
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(REQUEST_TIMEOUT))
            // The answer to a request is its status, whatever it is.
            .http_status_as_error(false)
            // A redirect would send the token to a place it was not given for.
            .max_redirects(0)
            .build()
            .into();
        Ok(Self {
            agent,
            base,
            bearer: Bearer(format!("Bearer {}", token.0)),
            token,
        })
    }

    /// Wait for a workload. `None` when the control plane had none within its wait. `slots` is
    /// how many workloads this worker may run at once, which the control plane shows.
    ///
    /// # Errors
    ///
    /// As for any request.
    pub fn claim(&self, slots: usize) -> Result<Option<Claim>, Failure> {
        let body = format!("{{\"slots\":{slots}}}");
        let (status, body) = self.post("/worker/claim", Some(&body))?;
        if status == 204 {
            return Ok(None);
        }
        let answer: Value = serde_json::from_str(&body)
            .map_err(|_| Failure::Trouble("the claim's answer is not JSON".to_owned()))?;
        let workload = answer["workload"].clone();
        let id = workload["id"]
            .as_str()
            .filter(|id| !id.is_empty())
            .ok_or_else(|| Failure::Refused("the claim's workload has no id".to_owned()))?
            .to_owned();
        let seconds = answer["lease"]["seconds"]
            .as_u64()
            .filter(|seconds| *seconds > 0)
            .ok_or_else(|| Failure::Refused("the claim says nothing about its lease".to_owned()))?;
        Ok(Some(Claim {
            workload,
            id,
            lease: Duration::from_secs(seconds),
            next_sequence: answer["nextSequence"].as_u64().unwrap_or(1).max(1),
        }))
    }

    /// Send events, each already a line of JSON, in order.
    ///
    /// # Errors
    ///
    /// As for any request.
    pub fn events(&self, id: &str, lines: &[String]) -> Result<(), Failure> {
        let events: Vec<String> = lines
            .iter()
            // A line that is not JSON is sent as the text it is, and the control plane says it
            // is not an event, as the local runner does.
            .map(|line| {
                if serde_json::from_str::<Value>(line).is_ok() {
                    line.clone()
                } else {
                    Value::String(line.clone()).to_string()
                }
            })
            .collect();
        let body = format!("{{\"events\":[{}]}}", events.join(","));
        self.post(&format!("/worker/workloads/{id}/events"), Some(&body))
            .map(|_| ())
    }

    /// Say this executor is still there.
    ///
    /// # Errors
    ///
    /// As for any request.
    pub fn heartbeat(&self, id: &str) -> Result<(), Failure> {
        self.post(&format!("/worker/workloads/{id}/heartbeat"), None)
            .map(|_| ())
    }

    /// Send the run record, at the end.
    ///
    /// # Errors
    ///
    /// As for any request.
    pub fn record(&self, id: &str, record: &str) -> Result<(), Failure> {
        self.post(&format!("/worker/workloads/{id}/record"), Some(record))
            .map(|_| ())
    }

    /// Give the workload back: its operator stopped this machine's executor, so the run did not
    /// end and nothing about the ticket changed.
    ///
    /// # Errors
    ///
    /// As for any request.
    pub fn release(&self, id: &str) -> Result<(), Failure> {
        self.post(&format!("/worker/workloads/{id}/release"), None)
            .map(|_| ())
    }

    fn post(&self, path: &str, body: Option<&str>) -> Result<(u16, String), Failure> {
        let request = self
            .agent
            .post(format!("{}{path}", self.base))
            .header("Authorization", &self.bearer.0);
        let sent = match body {
            Some(body) => request.content_type("application/json").send(body),
            None => request.send_empty(),
        };
        let mut answer = sent.map_err(|error| Failure::Trouble(self.scrub(&error.to_string())))?;
        let status = answer.status().as_u16();
        let text = answer
            .body_mut()
            .read_to_string()
            .map_err(|error| Failure::Trouble(self.scrub(&error.to_string())))?;
        match status {
            200..=204 => Ok((status, text)),
            409 => Err(Failure::Lost),
            401 | 403 => Err(Failure::Denied),
            408 | 425 | 429 | 500..=599 => Err(Failure::Trouble(format!(
                "the control plane answered {status}: {}",
                self.scrub(&shown(&text))
            ))),
            _ => Err(Failure::Refused(format!(
                "the control plane answered {status}: {}",
                self.scrub(&shown(&text))
            ))),
        }
    }

    /// Text that is to be shown, with the token taken out of it, wherever it came from.
    fn scrub(&self, text: &str) -> String {
        if self.token.is_in(text) {
            text.replace(&self.token.0, "[token]")
        } else {
            text.to_owned()
        }
    }
}

fn shown(text: &str) -> String {
    let line = text.lines().next().unwrap_or_default();
    line.chars().take(SHOWN).collect()
}

/// Whether the host (with its port, if any) is this machine, which a plain http connection
/// does not leave.
fn is_this_machine(host: &str) -> bool {
    let name = if let Some(inner) = host.strip_prefix('[') {
        inner.split(']').next().unwrap_or_default()
    } else {
        host.split(':').next().unwrap_or_default()
    };
    matches!(name, "localhost" | "127.0.0.1" | "::1")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client(address: &str) -> Result<Client, String> {
        Client::new(address, Token::new("hdt_x".to_owned()))
    }

    #[test]
    fn the_token_is_never_shown_by_debug() {
        let shown = format!("{:?}", Token::new("hdt_secret".to_owned()));
        assert!(!shown.contains("hdt_secret"), "{shown}");
    }

    #[test]
    fn https_is_accepted_and_plain_http_only_to_this_machine() {
        assert!(client("https://control.example.com").is_ok());
        assert!(client("https://control.example.com:8443/base/").is_ok());
        assert!(client("http://127.0.0.1:4000").is_ok());
        assert!(client("http://localhost:4000").is_ok());
        assert!(client("http://[::1]:4000").is_ok());
        let error = client("http://control.example.com").expect_err("refused");
        assert!(error.contains("https"), "{error}");
        assert!(client("http://127.0.0.1.example.com").is_err());
    }

    #[test]
    fn what_is_not_an_address_is_refused() {
        assert!(client("control.example.com").is_err());
        assert!(client("ftp://control.example.com").is_err());
        assert!(client("https://").is_err());
        assert!(client("https://user:pass@control.example.com").is_err());
    }

    #[test]
    fn the_token_is_taken_out_of_text_that_is_shown() {
        let client = client("https://control.example.com").expect("a client");
        assert_eq!(client.scrub("bad hdt_x here"), "bad [token] here");
        assert_eq!(client.scrub("nothing"), "nothing");
    }
}
