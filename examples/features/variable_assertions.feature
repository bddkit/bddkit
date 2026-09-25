@demo @variables
Feature: assertions over a variable's text

  # Text reaches a variable from a column, a cookie, a plugin or — as here —
  # a response. The response body's checks work over any of them.

  Scenario: substring and regex checks, and a regex capture into a variable
    When I request "/users/1" using HTTP GET
    Then the response code is 200
    And extract "email" from JSON as "email"
    And variable "email" should contain "@example.com"
    And variable "email" should not contain "@gmail"
    And variable "email" should match "^[a-z]+@example\.com$"
    And variable "email" should not match "\s"
    # The first capture group of the first match; a regex with no group is an error.
    And extract "^([a-z]+)@" from variable "email" as "login"
    And variable "login" should be equal to "leanne"
    Given set variable "nickname" to ""
    Then variable "nickname" should be empty

  Scenario: a variable holding a JSON document
    When I request "/users/1" using HTTP GET
    Then the response code is 200
    # A non-scalar node is extracted as its JSON text.
    And extract "address" from JSON as "address"
    And variable "address" should contain JSON:
      """
      {"geo": {"lat": "@regExp(/^-?\\d+\\.\\d+$/)"}}
      """
    And variable "address" should equal JSON:
      """
      {"city": "Gwenborough", "geo": {"lat": "-37.3159", "lng": "81.1496"}}
      """
    And variable "address" should not contain JSON:
      """
      {"city": "Paris"}
      """
    And extract "geo.lat" from variable "address" as JSON as "lat"
    And variable "lat" should be equal to "-37.3159"

  Scenario: a macro publishes what it extracts with `global`
    When I request "/users/1" using HTTP GET
    Then the response code is 200
    And extract "address" from JSON as "address"
    # The macro declares no exports; only its two `global` extracts outlive it.
    When I read the city and latitude of "address"
    Then variable "city" should be equal to "Gwenborough"
    And variable "latDegrees" should be equal to "-37"
