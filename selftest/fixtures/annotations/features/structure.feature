# The comment of the file.
Feature: Annotations on purpose
  # One scenario that fails halfway, so the stream shows what a report gets
  # for a scenario that stopped: its structure is still all there.

  # The comment of the scenario.
  Scenario: notes, a disabled step and sections around a failure
    The description is the plain text under the Scenario line.
    # A comment inside the description is not part of it.
    It may run over several lines.

    # Setup
    # An order paid by card.

    Given set variable "x" to "1"
    # The note of the next step.
    When set variable "y" to "2"
    # Disabled until #123 is fixed.
    # Then variable "y" should be equal to "3"

    # Checks

    Then variable "x" should be equal to "nothing"
    # Disabled after the failure.
    # Then variable "x" should be equal to "1"

    # After the failure

    And variable "y" should be equal to "2"

    # The end
