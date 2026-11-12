# Lists the list fixture and writes nothing; it joins the serial chain like the
# other selftest features, which share selftest/out.
@serial(selftest)
Feature: bddkit list shows what a run would run and keeps the masking

  Scenario: the listing holds the example rows and no config secret
    When I run the command "${BDDKIT_BIN:-target/debug/bddkit} list --config selftest/fixtures/list/bddkit.yaml"
    Then the command exit code is 0
    And the command output contains "names.feature:4  sign in with ***"
    And the command output contains "names.feature:13  row <n> [2]"
    And the command output does not contain "list-secret-value-8"

  Scenario: the JSON listing carries the stream's field names and no config secret
    When I run the command "${BDDKIT_BIN:-target/debug/bddkit} list --json --config selftest/fixtures/list/bddkit.yaml"
    Then the command exit code is 0
    And the command output contains "row_line"
    And the command output contains "examples"
    And the command output contains "@listed"
    And the command output does not contain "list-secret-value-8"

  Scenario: a path that does not exist is exit 2 with an error object
    When I run the command "${BDDKIT_BIN:-target/debug/bddkit} list --json --config selftest/fixtures/list/bddkit.yaml no/such/dir"
    Then the command exit code is 2
    And the command output contains "error"
    And the command output contains "no/such/dir"
    And the command error output contains "nothing listed"
