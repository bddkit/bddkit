Feature: A reusable setup that composes another include from its own directory
  @exports(inner)
  Scenario: inner
    Given set variable "inner" to "i"

  @exports(inner)
  Scenario: outer
    Given I include "lib.feature" scenario "inner"
