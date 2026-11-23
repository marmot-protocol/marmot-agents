//! The model double (`tools/model_double/double.py`), as the end-to-end scenarios run it: a local
//! server that speaks the providers' wire formats, so a real harness runs with only its model
//! faked. A scenario states what the model does (scripted), or replays a cassette recorded from a
//! real model, and checks at the end that every request the harness made was answered.

#![allow(dead_code)]

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};

/// A running double, stopped when dropped, and its container, if any, removed.
pub struct Double {
    /// Its address, `http://127.0.0.1:<port>`.
    pub url: String,
    /// The container a recording double runs in, by name.
    pub container: Option<String>,
    _child: Child,
    /// Declared after the child, so the client is stopped first.
    _removal: Option<Container>,
    /// The script, or a recording's output folder, kept while the double runs.
    _files: tempfile::TempDir,
}

/// The repository's top folder.
pub fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

impl Double {
    /// A double that answers as `script` says (the script's form is in `double.py`).
    pub async fn scripted(script: &Value) -> Self {
        Self::scripted_with(script, &[]).await
    }

    /// A scripted double that keeps each request's whole body, for `bodies`.
    pub async fn scripted_keeping_bodies(script: &Value) -> Self {
        Self::scripted_with(script, &["--keep-bodies"]).await
    }

    async fn scripted_with(script: &Value, args: &[&str]) -> Self {
        let files = tempfile::tempdir().unwrap();
        let path = files.path().join("script.json");
        std::fs::write(&path, script.to_string()).unwrap();
        let mut command = Command::new("python3");
        command
            .arg(repository().join("tools/model_double/double.py"))
            .arg("scripted")
            .arg(&path)
            .args(args);
        Self::start(command, files).await
    }

    /// The whole body of each model request (not a model list) the harness made, oldest first,
    /// from a double that keeps them.
    pub async fn bodies(&self) -> Vec<String> {
        let log = self.log().await;
        log.iter()
            .map(|e| e["body"].as_str().expect("kept bodies").to_string())
            .collect()
    }

    /// A double that answers from the cassette at `cassette`, and refuses any request it holds no
    /// answer for.
    pub async fn replay(cassette: &Path) -> Self {
        let files = tempfile::tempdir().unwrap();
        let mut command = Command::new("python3");
        command
            .arg(repository().join("tools/model_double/double.py"))
            .arg("replay")
            .arg(cassette);
        Self::start(command, files).await
    }

    /// A double that records: it sends each request on to the real model `model` at `upstream`, an
    /// OpenAI-compatible endpoint, and writes the cassette to `out`. It runs in a container that
    /// has the key file mounted read-only, and nothing else of it: the key never reaches this
    /// process, a command line or the environment (D-071).
    pub async fn record(
        key_file: &Path,
        upstream: &str,
        model: &str,
        out: &Path,
        harness: (&str, &str, &str),
    ) -> Self {
        let files = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(out).unwrap();
        let (name, version, format) = harness;
        let container = format!(
            "marmot-agent-model-double-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        // Made before the container, so a panic while it starts removes it too.
        let removal = Container(container.clone());
        let mut command = Command::new("docker");
        command
            .args(["run", "--rm", "-i", "--name", &container])
            .args(["--network", "host", "--user"])
            .arg(format!("{}:{}", uid(), gid()))
            .arg("-v")
            .arg(format!("{}:/key:ro", key_file.display()))
            .arg("-v")
            .arg(format!(
                "{}:/double:ro",
                repository().join("tools/model_double").display()
            ))
            .arg("-v")
            .arg(format!("{}:/out", out.display()))
            .args([
                "python:3.14.10-slim-bookworm",
                "python3",
                "-I",
                "/double/double.py",
                "record",
                "/out/cassette.jsonl",
                "--key-file",
                "/key",
                "--upstream",
                upstream,
                "--model",
                model,
                "--harness",
                name,
                "--version",
                version,
                "--format",
                format,
            ]);
        let mut double = Self::start(command, files).await;
        double.container = Some(container);
        double._removal = Some(removal);
        double
    }

    async fn start(mut command: Command, files: tempfile::TempDir) -> Self {
        // With MODEL_DOUBLE_VERBOSE set, it prints each request's key as it answers, to see what a
        // harness asked when a scenario fails.
        if std::env::var_os("MODEL_DOUBLE_VERBOSE").is_some() {
            command.arg("--verbose");
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .expect("the model double starts");
        let stdout = child.stdout.take().unwrap();
        let mut lines = BufReader::new(stdout).lines();
        let url = lines
            .next_line()
            .await
            .unwrap()
            .expect("the model double names its address");
        Self {
            url,
            container: None,
            _child: child,
            _removal: None,
            _files: files,
        }
    }

    /// What the harness asked, oldest first: each request's key (`double.py`), and `unmatched`
    /// on one no answer was found for.
    pub async fn log(&self) -> Vec<Value> {
        let address = self.url.trim_start_matches("http://");
        let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
        let request =
            format!("GET /_double/log HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n");
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).await.unwrap();
        let (_, body) = response.split_once("\r\n\r\n").unwrap();
        serde_json::from_str(body).unwrap()
    }

    /// Checks that every request the harness made was answered, and gives back how many.
    pub async fn all_answered(&self) -> usize {
        let log = self.log().await;
        let unanswered: Vec<&Value> = log
            .iter()
            .filter(|e| e.get("unmatched").is_some() || e.get("error").is_some())
            .collect();
        assert!(
            unanswered.is_empty(),
            "the model double had no answer for {unanswered:#?}"
        );
        log.len()
    }
}

/// A container, by name, removed when dropped: on success, failure and panic alike. Killing
/// the `docker run` client alone leaves the container running, with the key mounted.
struct Container(String);

impl Drop for Container {
    fn drop(&mut self) {
        let _ = std::process::Command::new("docker")
            .args(["rm", "-f", &self.0])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

fn uid() -> String {
    id("-u")
}

fn gid() -> String {
    id("-g")
}

fn id(flag: &str) -> String {
    let out = std::process::Command::new("id").arg(flag).output().unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}
