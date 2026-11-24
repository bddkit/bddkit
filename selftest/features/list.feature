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

  # The broken file is written here, not kept under selftest/: the unit tests
  # parse every .feature in the repository and it must not parse.
  Scenario: a feature file that does not parse is listed with its error, and a run over it is exit 2
    When I run the command "rm -rf selftest/out/list-broken && mkdir -p selftest/out/list-broken/features && printf 'Feature: fine\n  Scenario: still listed\n    When I sleep 0 seconds\n' > selftest/out/list-broken/features/ok.feature && printf 'Feature: bad\n  Scenario: s\n    When I sleep 0 seconds\n  this is not gherkin\n' > selftest/out/list-broken/features/bad.feature && printf 'paths: [features]\nresources: {}\n' > selftest/out/list-broken/bddkit.yaml"
    Then the command exit code is 0
    When I run the command "${BDDKIT_BIN:-target/debug/bddkit} list --json --config selftest/out/list-broken/bddkit.yaml"
    Then the command exit code is 0
    And the command output contains "still listed"
    And the command output contains "bad.feature"
    And the command output contains "Error at 4:20: unknown keyword"
    When I run the command "${BDDKIT_BIN:-target/debug/bddkit} run --config selftest/out/list-broken/bddkit.yaml"
    Then the command exit code is 2
    And the command error output contains "failed to parse"

  # Written here for the same reason: a typo'd keyword must not parse, a BOM must.
  Scenario: a typo'd keyword is listed with its error and a BOM file is listed clean
    When I run the command "rm -rf selftest/out/list-typo && mkdir -p selftest/out/list-typo/features && printf '\357\273\277Feature: bom\n  Scenario: bom listed\n    When I sleep 0 seconds\n' > selftest/out/list-typo/features/bom.feature && printf 'Feature: typo\n  Scenaro: s\n    When I sleep 0 seconds\n' > selftest/out/list-typo/features/typo.feature && printf 'paths: [features]\nresources: {}\n' > selftest/out/list-typo/bddkit.yaml"
    Then the command exit code is 0
    When I run the command "${BDDKIT_BIN:-target/debug/bddkit} list --json --config selftest/out/list-typo/bddkit.yaml"
    Then the command exit code is 0
    And the command output contains "bom listed"
    And the command output contains "`Scenaro:` is not a keyword"
