@vars
Feature: variables

  Scenario: setting a variable and substituting it into a path
    Given set variable "path" to "/ping"
    And the "Accept" request header is "application/json"
    When I request "<<path>>" using HTTP GET
    Then the response code is 200

  Scenario: extracting from JSON and comparing
    When I request "/ping" using HTTP GET
    Then extract "version" from JSON as "v"
    And variable "v" should be equal to "3"
    And variable "v" should not be equal to "4"

  Scenario: extracting from cookies
    Given the "Content-Type" request header is "application/json"
    And the request body is:
      """
      {"password": "correct"}
      """
    When I request "/login" using HTTP POST
    Then extract "jwt_token" from cookies as "token"
    And variable "token" should be equal to "tok-abc"

  Scenario: unique values differ
    Given set variable "a" to "<<unique(token)>>"
    And set variable "b" to "<<unique(token)>>"
    Then variable "a" should not be equal to "<<b>>"

  Scenario: variable survives the scenario boundary
    Then variable "a" should not be equal to ""

  Scenario: text assertions over a variable
    When I request "/ping" using HTTP GET
    Then extract "version" from JSON as "id"
    And variable "id" should match "^\d+$"
    And variable "id" should not match "[a-z]"
    And set variable "greeting" to "hello, world"
    And variable "greeting" should contain "world"
    And variable "greeting" should not contain "bye"
    And set variable "blank" to ""
    And variable "blank" should be empty
    And extract "^(\w+)," from variable "greeting" as "first"
    And variable "first" should be equal to "hello"

  Scenario: JSON assertions over a variable
    Given the "Content-Type" request header is "application/json"
    And the request body is:
      """
      {"user": {"id": 7, "roles": ["a", "b"]}}
      """
    When I request "/echo" using HTTP POST
    Then extract "received" from JSON as "doc"
    And variable "doc" should contain JSON:
      """
      {"user": {"id": "@variableType(int)", "roles": ["b"]}}
      """
    And variable "doc" should equal JSON:
      """
      {"user": {"id": 7, "roles": ["a", "b"]}}
      """
    And variable "doc" should not contain JSON:
      """
      {"user": {"id": 8}}
      """
    And extract "user.roles[1]" from variable "doc" as JSON as "role"
    And variable "role" should be equal to "b"
    And I pull the user id out of variable "doc"
    And variable "uid" should be equal to "7"
    And variable "digits" should be equal to "7"
