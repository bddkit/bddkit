# Writes selftest/out like the other features, so they run one after the other.
@serial(selftest)
Feature: evidence names the polling attempt that wrote it
  # Runs bddkit on selftest/fixtures/polling, whose one assertion polls a
  # process until it times out, then reads the numbers back from the stream.

  Scenario: the evidence of a polled step is numbered from 1
    When I run the command "rm -rf selftest/out && mkdir -p selftest/out && ${BDDKIT_BIN:-target/debug/bddkit} run --config selftest/fixtures/polling/bddkit.yaml --events selftest/out/events.ndjson --evidence"
    Then the command exit code is 1
    When I run the command "jq -r -f selftest/fixtures/polling/attempts.jq selftest/out/events.ndjson | uniq | head -n 2 | tr '\n' '|'"
    Then the command output contains "1|2|"
