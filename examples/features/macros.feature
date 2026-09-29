@demo @macros
Feature: domain steps declared in YAML

  # These steps are not built into the binary — they come from
  # examples/macros/posts.yaml and are matched exactly like built-in ones.
  # `I create a post with title` posts a body, asserts 201 and extracts the
  # new id, all behind one line a non-developer can read.

  Scenario: a macro with a parameter and one export
    When I log in as "demo" with password "s3cr3t"
    Then variable "sessionId" should be equal to "s3cr3t-session"

  Scenario: a macro that calls another macro
    When I publish a post titled "nested" as "demo"
    Then variable "sessionId" should be equal to "s3cr3t-session"
    And variable "lastPostId" should be equal to "101"

  Scenario: what a macro does not export stays inside it
    # `I log in as` sets `user` and `password` in its own frame and exports
    # only `sessionId`, so nothing else leaks out here.
    When I log in as "demo" with password "s3cr3t"
    Then Show all variables

  Scenario Outline: the same macro over a table of inputs
    When I create a post with title "<title>"
    Then variable "lastPostId" should be equal to "<expected_id>"

    Examples:
      | title       | expected_id |
      | first post  | 101         |
      | second post | 101         |
      | ünïcödé 🎉  | 101         |

  Scenario: a macro template with an alternation and optional text
    When I note the article title "Hello"
    Then variable "notedTitle" should be equal to "Hello"
    When I note the post main title "Again"
    Then variable "notedTitle" should be equal to "Again"
