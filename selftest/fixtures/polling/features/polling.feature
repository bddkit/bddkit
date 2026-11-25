Feature: an eventual assertion that never passes
  Scenario: a process that stays silent
    Given I start the "slow" process running "sleep 5"
    And I expect the next assertion to pass within "1" seconds, checking every "400" milliseconds
    Then the "slow" process output contains "done"
