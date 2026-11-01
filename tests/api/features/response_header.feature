@api
Feature: response header text steps

  Scenario: a header repeated on two lines is checked on every line
    When I request "/links"
    Then the "X-Tag" response header is "b"
    And the "link" response header contains "</start.md>"
    And the "Link" response header matches "type=.text/markdown.$"
    And the "Link" response header does not contain "</nope>"
    And the "Link" response header does not match "^</nope>"

  Scenario: double quotes inside the expected text are kept
    When I request "/links"
    Then the "Link" response header contains "</llms.txt>; rel="describedby""

  Scenario: a missing header only satisfies the negative steps
    When I request "/links"
    Then the "X-Absent" response header does not contain "a"
    And the "X-Absent" response header does not match "a"
