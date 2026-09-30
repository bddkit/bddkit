@demo @variables
Feature: variables — setting, extracting, reusing

  Scenario: a variable set by hand, used in the path
    Given set variable "postId" to "1"
    When I request "/posts/<<postId>>" using HTTP GET
    Then the response code is 200
    And the response body contains JSON:
      """
      {"id": 1}
      """

  Scenario: extract from one response, use it in the next request
    Given the "Content-Type" request header is "application/json"
    And the request body is:
      """
      {"title": "chained", "body": "demo", "userId": 1}
      """
    When I request "/posts" using HTTP POST
    Then the response code is 201
    And extract "id" from JSON as "createdId"
    And variable "createdId" should not be equal to "1"
    # The variable survives into the next request of the same scenario…
    When I request "/posts/999" using HTTP GET
    Then the response code is 404
    And the response body contains JSON:
      """
      {"id": 999}
      """

  Scenario: …and into the next scenario of the same file
    # Variables live per feature FILE, so `createdId` above is still readable
    # here. HTTP state is not: the previous response is gone.
    Then variable "createdId" should be equal to "101"
    And Show "createdId" variable

  Scenario: unique data per run, echoed back by the server
    Given set variable "title" to "post-<<unique()>>"
    And the "Content-Type" request header is "application/json"
    And the request body is:
      """
      {"title": "<<title>>", "body": "demo", "userId": 1}
      """
    When I request "/echo" using HTTP POST
    Then the response code is 201
    And extract "title" from JSON as "echoed"
    # Same value out as went in — a fresh one on every run, so re-running the
    # suite never collides with data an earlier run left behind.
    And variable "echoed" should be equal to "<<title>>"
    And Show all variables

  Scenario: a variable filled from a response header's cookie
    Given the request form parameters are:
      | name | value |
      | user | demo  |
    When I request "/login" using HTTP POST
    Then the response code is 200
    And extract "session" from cookies as "sessionId"
    And the "Authorization" request header is "Bearer <<sessionId>>"
    When I request "/posts/1" using HTTP GET
    Then the response code is 200

  Scenario: variables in the typed positions of built-in steps
    # A typed position (an HTTP method, a status code) takes a literal of its
    # type or a whole <<variable>>; the variable's value is checked at run time.
    Given set variable "verb" to "PATCH"
    And set variable "ok" to "200"
    When I request "/posts/1" using HTTP <<verb>>
    Then the response code is <<ok>>
