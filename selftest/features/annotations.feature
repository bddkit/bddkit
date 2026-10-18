# Both files write selftest/out, so they run one after the other.
@serial(selftest)
Feature: Comments, disabled steps and sections reach the event stream
  # Each scenario runs bddkit on selftest/fixtures/annotations, whose one
  # scenario fails halfway, and reads the stream back through a jq filter that
  # puts the events of interest on one line. The point is what a report built
  # from the stream receives, in which order, with which numbers.

  Scenario: sections and disabled steps keep the order they are written in
    When I run the annotations fixture and read it through "order"
    Then the command output contains "section Setup|step_started set variable 'x' to '1'|step_started set variable 'y' to '2'|step_disabled variable 'y' should be equal to '3'|section Checks|"

  Scenario: they are still emitted after a failed step, between the skipped steps
    When I run the annotations fixture and read it through "order"
    Then the command output contains "step_started variable 'x' should be equal to 'nothing'|step_disabled variable 'x' should be equal to '1'|section After the failure|step_skipped variable 'y' should be equal to '2'|section The end|"

  Scenario: a closing note is a section after the last step
    When I run the annotations fixture and read it through "order"
    Then the command output matches "[|]section The end[|]$"

  Scenario: a disabled step has no number and does not shift the ones after it
    When I run the annotations fixture and read it through "numbers"
    Then the command output contains "step_started=0|step_started=1|step_disabled=-|step_started=2|step_disabled=-|step_skipped=3|"

  Scenario: a block right above an element is its comment
    When I run the annotations fixture and read it through "comments"
    Then the command output contains "file_started: The comment of the file.|"
    And the command output contains "scenario_started: The comment of the scenario.|"
    And the command output contains "step_started: The note of the next step.|"
    And the command output contains "step_disabled: Disabled until #123 is fixed.|"

  Scenario: the plain text under the Scenario line is its description
    When I run the annotations fixture and read it through "description"
    Then the command output contains "The description is the plain text under the Scenario line.|It may run over several lines.|"

  Scenario: a comment inside a description is neither description nor annotation
    When I run the annotations fixture and read it through "description"
    Then the command output does not contain "A comment inside"
    When I run the annotations fixture and read it through "comments"
    Then the command output does not contain "A comment inside"
    When I run the annotations fixture and read it through "order"
    Then the command output does not contain "A comment inside"
