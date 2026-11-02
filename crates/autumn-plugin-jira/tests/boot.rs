//! EX2: boots a real app (not `TestApp`) with config files and env vars.
//!
//! The test runs this test binary again as a child process, with a clean
//! env. So it changes no env in this process.

#![allow(clippy::unwrap_used, clippy::expect_used, reason = "tests")]
#![allow(clippy::print_stderr, reason = "the child reports its own failure")]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use autumn_jira_fake::{DEFAULT_API_TOKEN, DEFAULT_EMAIL, FakeJira};
use autumn_plugin_jira::{JiraPlugin, JiraService, webhook_endpoint_toml};
use autumn_web::webhook::hmac_sha256_hex;

const CHILD_MARK: &str = "AUTUMN_PLUGIN_JIRA_BOOT_CHILD";
const SECRET: &str = "boot-webhook-secret-0123456789abcdef";
/// When set, the child writes a long log before it starts.
const NOISE_MARK: &str = "AUTUMN_PLUGIN_JIRA_BOOT_NOISE";

#[autumn_web::get("/")]
async fn index() -> &'static str {
    "app"
}

/// The server. It runs only in the child process. Its Jira is `FakeJira`,
/// so no test calls a real Jira.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "runs only as the child of the boot tests"]
async fn boot_child_server() {
    if std::env::var(CHILD_MARK).is_err() {
        return;
    }
    if std::env::var(NOISE_MARK).is_ok() {
        // More output than one pipe buffer (64 KiB on Linux), as a long
        // log gives.
        for n in 0..4096 {
            eprintln!("boot child log line {n:05} {}", "x".repeat(40));
        }
    }
    Box::pin(
        autumn_web::app()
            .routes(autumn_web::routes![index])
            .plugin(
                JiraPlugin::new()
                    .transport(Arc::new(FakeJira::new()))
                    .webhook_route(),
            )
            // autumn runs state initializers before job workers and startup
            // hooks. The service must be ready here, so a `#[job]` can use it.
            .state_initializer(|state| {
                if JiraService::from_state(state).is_none() {
                    eprintln!("no JiraService before job workers start");
                }
            })
            .run(),
    )
    .await;
}

struct KillOnDrop(Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn free_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().port()
}

fn app_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("jira-boot-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let toml = format!(
        "[jira.api_token]\nsite = \"acme\"\nemail_env = \"BOOT_JIRA_EMAIL\"\ntoken_env = \"BOOT_JIRA_TOKEN\"\n\n{}",
        webhook_endpoint_toml("/jira/webhook", "BOOT_JIRA_WEBHOOK_SECRET")
    );
    std::fs::write(dir.join("autumn.toml"), toml).unwrap();
    dir
}

/// Starts the child with a clean env, so the `AUTUMN_*` vars, `.env` or
/// `autumn.toml` of a developer cannot change the result.
fn spawn(dir: &Path, port: u16, env: &[(&str, &str)]) -> KillOnDrop {
    let mut cmd = Command::new(std::env::current_exe().unwrap());
    cmd.args([
        "--exact",
        "boot_child_server",
        "--include-ignored",
        "--nocapture",
    ])
    .env_clear()
    .env("PATH", std::env::var_os("PATH").unwrap_or_default())
    .env("AUTUMN_MANIFEST_DIR", dir)
    .current_dir(dir)
    .env(CHILD_MARK, "1")
    .env("AUTUMN_SERVER__PORT", port.to_string())
    .env("AUTUMN_SERVER__HOST", "127.0.0.1")
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    for (k, v) in env {
        cmd.env(k, v);
    }
    KillOnDrop(cmd.spawn().unwrap())
}

fn output_of(child: &mut Child) -> String {
    let mut out = String::new();
    if let Some(mut e) = child.stderr.take() {
        std::io::Read::read_to_string(&mut e, &mut out).unwrap();
    }
    if let Some(mut o) = child.stdout.take() {
        std::io::Read::read_to_string(&mut o, &mut out).unwrap();
    }
    out
}

#[tokio::test(flavor = "multi_thread")]
async fn missing_secret_exits_and_names_the_variable() {
    let port = free_port();
    let dir = app_dir("missing");
    let mut child = spawn(
        &dir,
        port,
        &[
            ("BOOT_JIRA_EMAIL", DEFAULT_EMAIL),
            ("BOOT_JIRA_WEBHOOK_SECRET", SECRET),
        ],
    );
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            break status;
        }
        assert!(
            start.elapsed() < Duration::from_secs(60),
            "the app did not stop"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    let out = output_of(&mut child.0);
    assert!(!status.success(), "{out}");
    assert!(out.contains("BOOT_JIRA_TOKEN"), "{out}");
    assert!(!out.contains(SECRET), "{out}");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_app_boots_and_takes_a_signed_post() {
    let port = free_port();
    let dir = app_dir("ok");
    // R9-test-quality-4: trace logs fill a pipe buffer. The test must not
    // depend on a small log volume.
    let mut child = spawn(
        &dir,
        port,
        &[
            ("BOOT_JIRA_EMAIL", DEFAULT_EMAIL),
            ("BOOT_JIRA_TOKEN", DEFAULT_API_TOKEN),
            ("BOOT_JIRA_WEBHOOK_SECRET", SECRET),
            ("AUTUMN_LOG__LEVEL", "trace"),
            (NOISE_MARK, "1"),
        ],
    );
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let base = format!("http://127.0.0.1:{port}");
    let start = Instant::now();
    loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            panic!(
                "the app stopped at boot ({status}):\n{}",
                output_of(&mut child.0)
            );
        }
        assert!(
            start.elapsed() < Duration::from_secs(60),
            "the app did not answer"
        );
        if let Ok(res) = client.get(format!("{base}/")).send().await
            && res.status().as_u16() == 200
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis();
    let body = format!(
        r#"{{"timestamp":{ts},"webhookEvent":"jira:issue_updated","issue":{{"id":"10001","key":"OPS-1","fields":{{}}}}}}"#
    );
    let signature = format!(
        "sha256={}",
        hmac_sha256_hex(SECRET.as_bytes(), body.as_bytes())
    );
    let send = |id: &'static str, signature: String| {
        client
            .post(format!("{base}/jira/webhook"))
            .header("X-Hub-Signature", signature)
            .header("X-Atlassian-Webhook-Identifier", id)
            .header("content-type", "application/json")
            .body(body.clone())
            .send()
    };
    assert_eq!(
        send("boot-1", signature.clone())
            .await
            .unwrap()
            .status()
            .as_u16(),
        200
    );
    assert_eq!(
        send("boot-1", signature).await.unwrap().status().as_u16(),
        409
    );
    assert_eq!(
        send("boot-2", "sha256=00".to_owned())
            .await
            .unwrap()
            .status()
            .as_u16(),
        401
    );
    let health = client
        .get(format!("{base}/actuator/health"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(health.contains("\"jira\""), "{health}");
    assert!(
        !health.contains(DEFAULT_API_TOKEN) && !health.contains(DEFAULT_EMAIL),
        "{health}"
    );
    child.0.kill().unwrap();
    let out = output_of(&mut child.0);
    assert!(!out.contains("no JiraService"), "{out}");
    assert!(
        !out.contains(SECRET) && !out.contains(DEFAULT_API_TOKEN),
        "{out}"
    );
}
