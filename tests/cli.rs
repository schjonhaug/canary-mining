use assert_cmd::Command;
use predicates::str::contains;

#[test]
fn example_config_prints_regtest_config() {
    let mut cmd = Command::cargo_bin("canary-mining").unwrap();
    cmd.args(["example-config", "--network", "regtest"])
        .assert()
        .success()
        .stdout(contains("network = \"regtest\""))
        .stdout(contains("[bitcoin_core]"));
}
