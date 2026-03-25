mod common;

use assert_cmd::Command;
use predicates::prelude::predicate;
use tempfile::TempDir;

fn cmd() -> Command {
    Command::cargo_bin("rara-trading").expect("binary should exist")
}

#[test]
fn hello_default_name() {
    cmd()
        .arg("hello")
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""greeting":"Hello, world!"#));
}

#[test]
fn hello_custom_name() {
    cmd()
        .args(["hello", "Alice"])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""greeting":"Hello, Alice!"#));
}

#[test]
fn config_list() {
    let tmp = TempDir::new().expect("failed to create temp dir");
    cmd()
        .env("APP_DATA_DIR", tmp.path())
        .args(["config", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""action":"config_list"#));
}

#[test]
fn config_set_then_get() {
    let tmp = TempDir::new().expect("failed to create temp dir");
    let dir = tmp.path();

    // Set a value
    cmd()
        .env("APP_DATA_DIR", dir)
        .args(["config", "set", "agent.backend", "gemini"])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""ok":true"#));

    // Get the value back
    cmd()
        .env("APP_DATA_DIR", dir)
        .args(["config", "get", "agent.backend"])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""value":"gemini"#));
}

#[test]
fn config_set_unknown_key_fails() {
    let tmp = TempDir::new().expect("failed to create temp dir");
    cmd()
        .env("APP_DATA_DIR", tmp.path())
        .args(["config", "set", "no.such.key", "val"])
        .assert()
        .failure()
        .stdout(predicate::str::contains(r#""ok":false"#));
}

#[test]
fn config_get_unknown_key_fails() {
    let tmp = TempDir::new().expect("failed to create temp dir");
    cmd()
        .env("APP_DATA_DIR", tmp.path())
        .args(["config", "get", "no.such.key"])
        .assert()
        .failure()
        .stdout(predicate::str::contains(r#""ok":false"#));
}
