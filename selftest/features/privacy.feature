# Both files write selftest/out, so they run one after the other.
@serial(selftest)
Feature: Secrets stay out of what bddkit prints and writes
  # Each scenario runs bddkit on selftest/fixtures/masking, whose scenarios all
  # fail on purpose, then reads back the console, the JUnit report and the event
  # stream. The secrets are the values in selftest/fixtures/masking/features.
  # The stream is read for its failure texts and for the descriptions in
  # `scenario_started`. A literal in a step's own text reaches `step_started`
  # before anything could register it (spec 7.11), which the last scenario pins.

  Scenario Outline: by default a secret appears in none of the three outputs
    When I run the masking fixture "bddkit" with options ""
    Then the command exit code is 1
    And the command output contains "***"
    And the command output does not contain "<secret>"
    And the command error output does not contain "<secret>"
    When I run the command "cat selftest/out/junit.xml && grep scenario_finished selftest/out/events.ndjson"
    Then the command output does not contain "<secret>"

    Examples: one rule each
      | secret                | rule                                    |
      | pin-secret-value-1    | a variable with the word secret         |
      | literal-key-value-3   | a secret parameter                      |
      | header-token-value-4  | a sensitive header                      |
      | query-token-value-5   | a sensitive query parameter             |
      | config-secret-value-6 | a config key with a field word          |

  Scenario: a value no rule names stays visible
    When I run the masking fixture "bddkit" with options ""
    Then the command output contains "gdpr-note-value-2"

  Scenario: words_only replaces the default word
    When I run the masking fixture "words" with options ""
    Then the command output does not contain "gdpr-note-value-2"
    And the command output contains "pin-secret-value-1"

  Scenario: the flag shows everything and says so
    When I run the masking fixture "bddkit" with options "--reveal"
    Then the command output contains "pin-secret-value-1"
    And the command output contains "query-token-value-5"
    And the command error output contains "masking is off"

  Scenario: a reveal committed in the config is overridden by the environment
    When I run the masking fixture "reveal" with options ""
    Then the command output contains "query-token-value-5"
    And the command error output contains "masking is off"
    Given the command environment variable "BDDKIT_PRIVACY_REVEAL" is "false"
    When I run the masking fixture "reveal" with options ""
    Then the command output does not contain "query-token-value-5"
    And the command error output does not contain "masking is off"

  Scenario: a value shorter than min_length is hidden by position, not by value
    When I run the masking fixture "bddkit" with options "--privacy-min-length 40"
    # The variable's value is too short to join the set, so it shows ...
    Then the command output contains "pin-secret-value-1"
    # ... while the query value in the error is hidden where it stands.
    And the command output does not contain "query-token-value-5"

  Scenario: a literal in a step's own text reaches the stream, a known limit
    # `step_started` is emitted before the step that would register the value.
    # If this ever fails, the limit was lifted: delete the scenario and the
    # matching line of README "Privacy" and spec 7.11.
    When I run the masking fixture "bddkit" with options ""
    And I run the command "grep step_started selftest/out/events.ndjson"
    Then the command output contains "pin-secret-value-1"

  Scenario: a secret quoted in a description is hidden in the stream
    # `scenario_started` carries the description; the config named this value
    # before any file ran, so the stream never holds it.
    When I run the masking fixture "bddkit" with options ""
    And I run the command "grep scenario_started selftest/out/events.ndjson"
    Then the command output contains "Its description quotes *** too."
    And the command output does not contain "config-secret-value-6"

  Scenario: --evidence writes the resolved slots and still hides the secret
    # `vars` shows what a slot resolved to; the one under a privacy word is `***`.
    When I run the masking fixture "bddkit" with options "--evidence"
    And I run the command "grep '\"vars\"' selftest/out/events.ndjson"
    Then the command output contains "***"
    And the command output does not contain "pin-secret-value-1"
    When I run the command "grep -v step_started selftest/out/events.ndjson"
    Then the command output does not contain "query-token-value-5"

  Scenario: --evidence failures writes a failed scenario's slots late, still hidden
    # Deferred: the values are held until the scenario fails, then written as a
    # `vars` event of their own, masked before they were held.
    When I run the masking fixture "bddkit" with options "--evidence failures"
    And I run the command "grep '\"type\":\"vars\"' selftest/out/events.ndjson"
    Then the command output contains "deferred"
    And the command output contains "***"
    And the command output does not contain "pin-secret-value-1"
    When I run the command "grep -c '\"evidence\":\"full\"' selftest/out/events.ndjson"
    Then the command output contains "7"

  Scenario: secrets in comments, disabled steps and sections are masked
    When I run the masking fixture "bddkit" with options ""
    And I run the command "grep -E '\"type\":\"(section|step_disabled)\"|\"comment\"' selftest/out/events.ndjson"
    Then the command output contains "This note quotes ***."
    And the command output contains "Disabled because *** is private."
    And the command output contains "This step note quotes ***."
    And the command output contains "Closing note quotes ***."
    And the command output does not contain "config-secret-value-6"
