@listed
Feature: names

  Scenario: sign in with list-secret-value-8
    When I run the command "true"

  Scenario Outline: row <n>
    When I run the command "true"

    Examples:
      | n |
      | 1 |
      | 2 |
