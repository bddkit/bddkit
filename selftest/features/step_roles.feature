# Writes selftest/out like the other features, so they run one after the other.
@serial(selftest)
Feature: the stream says what a step is and how often it was polled
  # Runs bddkit on selftest/fixtures/polling, whose one assertion polls a
  # process until it times out, and reads the steps back from the stream.

  Scenario: a role on every step and the attempts of the polled one, without evidence
    When I run the command "rm -rf selftest/out && mkdir -p selftest/out && ${BDDKIT_BIN:-target/debug/bddkit} run --config selftest/fixtures/polling/bddkit.yaml --events selftest/out/events.ndjson"
    Then the command exit code is 1
    When I run the command "jq -r -f selftest/fixtures/polling/roles.jq selftest/out/events.ndjson | tr '\n' '|'"
    Then the command output contains "0 action|1 action|2 assertion|2 within true|"
