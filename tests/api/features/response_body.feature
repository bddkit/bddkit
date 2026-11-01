@api
Feature: assertions on a raw (non-JSON) response body

  Scenario: substring and regex checks on an XML body
    When I request "/xml"
    Then the response code is 200
    And the response body contains "a@b.net"
    And the response body does not contain "z@z.net"
    And the response body matches "user id=.1."
    And the response body does not match "no-such-[0-9]+-pattern"

  Scenario: substring and regex checks work on any content type, not just XML
    When I request "/plain"
    Then the response code is 200
    And the response body contains "hello"
    And the response body does not contain "goodbye"
    And the response body matches "^hello$"
    And the response body does not match "^bye$"

    When I request "/html"
    Then the response code is 200
    And the response body contains "Hello"
    And the response body matches "<h[0-9] "

  Scenario: element checks on an XML body, via XPath
    When I request "/xml"
    Then the response code is 200
    And the response body has element "//user[@id='2']/email" with text "c@d.net"
    And the response body does not have element "//user[@id='99']"
    And extract "//user[1]/@id" from response body as "firstUserId"
    And variable "firstUserId" should be equal to "1"

  Scenario: element checks on an HTML body, via CSS selectors
    When I request "/html"
    Then the response code is 200
    And the response body has element "h1#title" with text "Hello"
    And the response body does not have element ".missing"
    And extract "p.msg" from response body as "message"
    And variable "message" should be equal to "World"

  Scenario: CSS selectors tolerate real-world HTML, not just strict XHTML
    When I request "/html-loose"
    Then the response code is 200
    And the response body has element "div.box" with text "Loose"
