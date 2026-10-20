# Writes selftest/out like the other features, so they run one after the other.
@serial(selftest)
Feature: A failed scenario says why, in every output
  # The masking fixture fails on purpose. What matters here is that the class of
  # each failure reaches the console summary, the JUnit report and the stream.

  Scenario: the class is in the summary, the JUnit type and the stream
    When I run the masking fixture "bddkit" with options ""
    Then the command exit code is 1
    And the command output contains "failed: 7 (5 assertion, 1 transport, 1 input)"
    When I run the command "grep -c '<failure type=\"[a-z_]*\"' selftest/out/junit.xml"
    Then the command output contains "7"
    When I run the command "grep scenario_finished selftest/out/events.ndjson"
    Then the command output contains "failure_kind"
    And the command output contains "transport"
