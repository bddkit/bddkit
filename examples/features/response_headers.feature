Feature: Response headers carrying several values

  # `is` needs the whole value. `contains` and `matches` check part of it, and
  # a header the server repeats (several Link lines) is checked on every line.

  Scenario: a Link header with quotes inside it
    When I request "/agent-hints"
    Then the response code is 200
    And the "Link" response header contains "</llms.txt>; rel="describedby""
    And the "Link" response header matches "rel=.alternate.; type=.text/markdown."
    And the "Link" response header does not contain "</private>"

  Scenario: each line of a repeated header is a value of its own
    When I request "/agent-hints"
    # Anchored: passes only if the server sent </start.md> as its own Link line.
    Then the "Link" response header matches "^</start.md>"
    And the "Link" response header matches "^</llms.txt>"

  Scenario: a Vary list whose order the server decides
    When I request "/agent-hints"
    Then the "Vary" response header contains "Accept-Encoding"
    And the "Vary" response header does not match "(?i)cookie"
