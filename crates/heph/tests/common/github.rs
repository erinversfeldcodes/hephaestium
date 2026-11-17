//! A stand-in for GitHub, shared by the tests that need one.
//!
//! GitHub is stood in for by a `gh` program first on the path, which answers
//! reads from recorded responses shaped like GitHub's real ones, records each
//! write with its method, endpoint and body, and logs what it was asked. That
//! proves the logic against those shapes. It does not prove that GitHub answers
//! that way: the commands are also run against real repositories.

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use serde_json::{Value, json};

use super::{Scratch, compliant};

/// The stand-in for `gh`: a directory with the program and its responses.
pub struct Gh {
    pub scratch: Scratch,
}

impl Gh {
    pub fn new(name: &str) -> Self {
        let scratch = Scratch::new(name);
        let program = scratch.write(
            "bin/gh",
            "#!/bin/sh\n\
             dir=\"$HEPH_TEST_GH_DIR\"\n\
             echo \"$*\" >> \"$dir/calls.log\"\n\
             [ \"$1\" = \"api\" ] || { echo \"gh: unsupported\" >&2; exit 2; }\n\
             shift\n\
             method=GET; endpoint=\"\"; input=0\n\
             while [ $# -gt 0 ]; do\n\
               case \"$1\" in\n\
                 -X) method=\"$2\"; shift 2;;\n\
                 --input) input=1; shift 2;;\n\
                 *) endpoint=\"$1\"; shift;;\n\
               esac\n\
             done\n\
             name=$(printf '%s' \"$endpoint\" | tr '/?=&' '____')\n\
             if [ \"$method\" != GET ]; then\n\
               body=\"\"; [ $input = 1 ] && body=$(cat)\n\
               printf '%s\\t%s\\t%s\\n' \"$method\" \"$endpoint\" \"$body\" >> \"$dir/writes.log\"\n\
               if [ -f \"$dir/data/$name.$method.err\" ]; then cat \"$dir/data/$name.$method.err\" >&2; exit 1; fi\n\
               echo '{}'; exit 0\n\
             fi\n\
             if [ -f \"$dir/data/$name.err\" ]; then cat \"$dir/data/$name.err\" >&2; exit 1; fi\n\
             if [ -f \"$dir/data/$name.json\" ]; then cat \"$dir/data/$name.json\"; exit 0; fi\n\
             echo 'gh: Not Found (HTTP 404)' >&2\n\
             exit 1\n",
        );
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755))
            .expect("to make the stand-in executable");
        std::fs::create_dir_all(scratch.0.join("data")).expect("a data directory");
        Self { scratch }
    }

    pub fn file_for(endpoint: &str) -> String {
        let name: String = endpoint
            .chars()
            .map(|character| match character {
                '/' | '?' | '=' | '&' => '_',
                other => other,
            })
            .collect();
        format!("data/{name}")
    }

    pub fn respond(&self, endpoint: &str, body: &Value) -> &Self {
        self.scratch.write(
            &format!("{}.json", Self::file_for(endpoint)),
            &body.to_string(),
        );
        self
    }

    pub fn respond_empty(&self, endpoint: &str) -> &Self {
        self.scratch
            .write(&format!("{}.json", Self::file_for(endpoint)), "");
        self
    }

    pub fn fail(&self, endpoint: &str, message: &str) -> &Self {
        self.scratch
            .write(&format!("{}.err", Self::file_for(endpoint)), message);
        self
    }

    pub fn calls(&self) -> String {
        std::fs::read_to_string(self.scratch.0.join("calls.log")).unwrap_or_default()
    }

    /// Every write the stand-in was asked for, as (method, endpoint, body).
    pub fn writes(&self) -> Vec<(String, String, String)> {
        std::fs::read_to_string(self.scratch.0.join("writes.log"))
            .unwrap_or_default()
            .lines()
            .map(|line| {
                let mut parts = line.splitn(3, '\t');
                (
                    parts.next().unwrap_or_default().to_owned(),
                    parts.next().unwrap_or_default().to_owned(),
                    parts.next().unwrap_or_default().to_owned(),
                )
            })
            .collect()
    }

    /// Make a write to an endpoint fail with a message.
    pub fn fail_write(&self, method: &str, endpoint: &str, message: &str) -> &Self {
        self.scratch.write(
            &format!("{}.{method}.err", Self::file_for(endpoint)),
            message,
        );
        self
    }

    pub fn bin(&self) -> PathBuf {
        self.scratch.0.join("bin")
    }
}

pub const REPO: &str = "owner/name";

pub fn endpoint(path: &str) -> String {
    format!("repos/{REPO}{path}")
}

pub fn admin_bypass() -> Value {
    json!([{"actor_id": 5, "actor_type": "RepositoryRole", "bypass_mode": "pull_request"}])
}

pub fn pull_request_rule(approvals: u64) -> Value {
    json!({"type": "pull_request", "parameters": {
        "required_approving_review_count": approvals,
        "dismiss_stale_reviews_on_push": true,
        "require_code_owner_review": false,
        "require_last_push_approval": true,
        "required_review_thread_resolution": true
    }})
}

pub fn checks_rule(contexts: &[&str]) -> Value {
    let checks: Vec<Value> = contexts.iter().map(|c| json!({"context": c})).collect();
    json!({"type": "required_status_checks", "parameters": {
        "required_status_checks": checks,
        "strict_required_status_checks_policy": true,
        "do_not_enforce_on_create": false
    }})
}

pub fn rule(kind: &str) -> Value {
    json!({"type": kind, "parameters": null})
}

/// A ruleset as GitHub returns it when asked for one by id.
pub fn ruleset(
    id: u64,
    enforcement: &str,
    target: &str,
    include: &[&str],
    bypass: &Value,
    rules: &[Value],
) -> Value {
    json!({
        "id": id, "name": format!("ruleset-{id}"), "enforcement": enforcement,
        "target": target, "bypass_actors": bypass,
        "conditions": {"ref_name": {"include": include, "exclude": []}},
        "rules": rules
    })
}

/// A ruleset that satisfies every ruleset item for answers asking for the `ci`
/// check and the admin bypass.
pub fn good_ruleset() -> Value {
    ruleset(
        1,
        "active",
        "branch",
        &["~DEFAULT_BRANCH"],
        &admin_bypass(),
        &[
            rule("deletion"),
            rule("non_fast_forward"),
            rule("required_linear_history"),
            pull_request_rule(1),
            checks_rule(&["ci"]),
        ],
    )
}

pub fn install_rulesets(gh: &Gh, rulesets: &[Value]) {
    let summary: Vec<Value> = rulesets
        .iter()
        .map(|r| {
            json!({"id": r["id"], "name": r["name"], "target": r["target"], "enforcement": r["enforcement"]})
        })
        .collect();
    gh.respond(&endpoint("/rulesets"), &json!(summary));
    for r in rulesets {
        gh.respond(&endpoint(&format!("/rulesets/{}", r["id"])), r);
    }
}

pub fn repo_json() -> Value {
    json!({
        "visibility": "public", "private": false, "default_branch": "main",
        "delete_branch_on_merge": true, "description": "A repository.",
        "security_and_analysis": {
            "secret_scanning": {"status": "enabled"},
            "secret_scanning_push_protection": {"status": "enabled"},
            "dependabot_security_updates": {"status": "enabled"}
        }
    })
}

/// GitHub, answering as it does for a repository that satisfies every item.
pub fn compliant_github(name: &str) -> Gh {
    let gh = Gh::new(name);
    gh.respond(&endpoint(""), &repo_json());
    gh.respond_empty(&endpoint("/vulnerability-alerts"));
    gh.respond(
        &endpoint("/automated-security-fixes"),
        &json!({"enabled": true, "paused": false}),
    );
    gh.respond(
        &endpoint("/code-scanning/default-setup"),
        &json!({"state": "configured"}),
    );
    gh.respond(&endpoint("/code-scanning/analyses?per_page=1"), &json!([]));
    install_rulesets(&gh, &[good_ruleset()]);
    gh
}

pub const ANSWERS: &str = "schema_version = 0\nlicence = \"mit\"\ncontact = \"security@example.org\"\n\
                       required_checks = [\"ci\"]\ndefault_branch_bypass = \"admin-pull-request\"\n";

pub fn repository(name: &str) -> Scratch {
    let scratch = compliant(name);
    scratch.write(".athenaeum/baseline.toml", ANSWERS);
    scratch
}
