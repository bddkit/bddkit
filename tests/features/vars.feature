@vars
Feature: variables

  Scenario: setting a variable and substituting it into another
    Given set variable "path" to "/ping"
    And set variable "copy" to "<<path>>"
    Then variable "copy" should be equal to "/ping"

  Scenario: unique values differ
    Given set variable "a" to "<<unique(token)>>"
    And set variable "b" to "<<unique(token)>>"
    Then variable "a" should not be equal to "<<b>>"

  Scenario: variable survives the scenario boundary
    Then variable "a" should not be equal to ""

  Scenario: text assertions over a variable
    Given set variable "id" to "3"
    Then variable "id" should match "^\d+$"
    And variable "id" should not match "[a-z]"
    And set variable "greeting" to "hello, world"
    And variable "greeting" should contain "world"
    And variable "greeting" should not contain "bye"
    And set variable "blank" to ""
    And variable "blank" should be empty
    And extract "^(\w+)," from variable "greeting" as "first"
    And variable "first" should be equal to "hello"

  Scenario: a macro template with optional text and an alternation
    When I remember the text "first" as a note
    Then variable "note" should be equal to "first"
    When I remember the value "second" as a new note
    Then variable "note" should be equal to "second"

  Scenario: a variable in a typed position
    Given set variable "expected" to "0"
    When I sleep <<expected>> seconds
    Then variable "expected" should be equal to "0"

  Scenario: a typed macro parameter, given a literal or a variable
    Then waiting "briefly" for 0 seconds
    Given set variable "n" to "0"
    Then waiting "briefly" for <<n>> seconds
