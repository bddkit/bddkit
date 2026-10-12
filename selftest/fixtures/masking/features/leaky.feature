Feature: Leaks on purpose
  # Every scenario fails, and every failure would print a known secret if
  # masking were broken. Nothing here needs a network: the one request goes to a
  # port nothing listens on, and the failure it raises is part of the point.

  Scenario: a variable with a privacy word
    Given set variable "secret_pin" to "pin-secret-value-1"
    Then variable "secret_pin" should be equal to "nothing"

  Scenario: a variable with another privacy word
    Given set variable "gdpr_note" to "gdpr-note-value-2"
    Then variable "gdpr_note" should be equal to "nothing"

  Scenario: a literal secret parameter
    Given I encrypt "payload" with AES using key "literal-key-value-3" as "c"

  Scenario: a header and a query token that never reach a server
    Given the "Authorization" request header is "Bearer header-token-value-4"
    And the query parameter "api_token" is "query-token-value-5"
    When I request "/ping"
    Then the response code is 200

  Scenario: a secret that only the config names
    Given set variable "note" to "config-secret-value-6"
    Then variable "note" should be equal to "nothing"
