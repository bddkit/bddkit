Feature: Hawk request signing

  Scenario: a raw JSON request carries a one-shot Hawk authorization header
    Given the "Content-Type" request header is "application/json; charset=utf-8"
    And the request body is:
      """
      {"code":"555555"}
      """
    And set variable "sessionId" to "session-1"
    And set variable "sessionKey" to "0123456789012345678901234567890123456789012345678901234567890123"
    And I sign the next request with Hawk id "<<sessionId>>" and key "<<sessionKey>>"
    When I request "/hawk" using HTTP POST
    Then the response code is 200
    And the JSON node "authorization" should exist
    And the response body contains JSON:
      """
      {"body":{"code":"555555"}}
      """
    # The signer is one-shot: repeating the request without re-signing must go
    # out unsigned, and the stub rejects a missing Authorization header.
    When I request "/hawk" using HTTP POST
    Then the response code is 400
