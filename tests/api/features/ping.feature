@api
Feature: basic HTTP steps

  Background:
    Given the "Accept" request header is "application/json"

  Scenario: GET without specifying a method
    When I request "/ping"
    Then the response code is 200
    And the response body contains JSON:
      """
      {"status": "ok"}
      """

  Scenario: POST with a body
    Given the "Content-Type" request header is "application/json"
    And the request body is:
      """
      {"name": "test"}
      """
    When I request "/echo" using HTTP POST
    Then the response code is 201
    And the response body contains JSON:
      """
      {"id": 42, "received": {"name": "test"}}
      """

  Scenario: array and matchers
    When I request "/users" using HTTP GET
    Then the response code is 200
    And the response body is a JSON array of length 2
    And the response body contains JSON:
      """
      [{"email": "@variableType(string)", "id": 1}]
      """

  Scenario: strict comparison
    When I request "/ping" using HTTP GET
    Then the response body equals JSON:
      """
      {"status": "ok", "version": 3}
      """

  Scenario: response header and node check
    Given the "Content-Type" request header is "application/json"
    And the request body is:
      """
      {"password": "correct"}
      """
    When I request "/login" using HTTP POST
    Then the response code is 200
    And the "x-trace" response header is "trace-1"
    And the JSON node "jwt" should exist

  Scenario: JSON node absence and null checks
    Given the "Content-Type" request header is "application/json"
    And the request body is:
      """
      {"name": "test", "deletedAt": null}
      """
    When I request "/echo" using HTTP POST
    Then the JSON node "received.password" should not exist
    And the JSON node "received.deletedAt" should be null
    And the JSON node "received.name" should not be null
    And the response body does not contain JSON:
      """
      {"received": {"password": "secret"}}
      """
    And the JSON node "received.name" should not contain "xyz"

  Scenario: error responses do not leak internals
    Given the "Content-Type" request header is "application/json"
    And the request body is:
      """
      {"password": "wrong"}
      """
    When I request "/login" using HTTP POST
    Then the JSON node "errorMessage" should not contain "SQL"

  Scenario: empty response body
    When I request "/no-content" using HTTP GET
    Then the response code is 204
    And the response body should be empty

  Scenario Outline: query parameter
    Given the query parameter "email" is "<email>"
    When I request "/users" using HTTP GET
    Then the response body is a JSON array of length <count>
    Examples:
      | email   | count |
      | a@b.net | 1     |
      | z@z.net | 2     |
