@api
Feature: debug printing of the response

  Scenario: print headers and a JSON response body
    When I request "/ping"
    Then the response code is 200
    And Print response headers
    And Print response body
    And Print response body as "status"

  Scenario: print an XML response and select by XPath
    When I request "/xml"
    Then the response code is 200
    And Print response body
    And Print response body as "//user[@id='2']/email"

  Scenario: print an HTML response and select by CSS
    When I request "/html"
    Then the response code is 200
    And Print response body
    And Print response body as "p"
