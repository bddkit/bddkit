Feature: Includes a file that itself includes a sibling
  Scenario: A depth-2 include resolves against the intermediate file
    When I include "../includes/lib.feature" scenario "outer"
    Then variable "inner" should be equal to "i"
