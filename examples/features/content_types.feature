@demo @content-types
Feature: responses that are not JSON

  Scenario Outline: the body comes back with the content type it claims
    Given the "Accept" request header is "<accept>"
    When I request "<path>" using HTTP GET
    Then the response code is 200
    And the "Content-Type" response header is "<content_type>"
    And the response body contains "<text>"
    And Print response body

    # <placeholder> is filled in when the file is parsed, once per row —
    # a different syntax from the runtime <<variable>>, on purpose.
    Examples:
      | path          | accept          | content_type              | text                                |
      | /posts/1.html | text/html       | text/html; charset=utf-8  | sunt aut facere repellat provident  |
      | /posts/1.xml  | application/xml | application/xml           | sunt aut facere repellat provident  |
      | /health       | text/plain      | text/plain; charset=utf-8 | OK                                  |

  Scenario: a form login hands back a session cookie
    Given the request form parameters are:
      | name     | value  |
      | user     | demo   |
      | password | s3cr3t |
    When I request "/login" using HTTP POST
    Then the response code is 200
    And the response body contains JSON:
      """
      {"status": "ok"}
      """
    And extract "session" from cookies as "sessionId"
    And variable "sessionId" should be equal to "s3cr3t-session"

  # The path is relative to this feature file. The mock only answers a request
  # whose Content-Type is multipart/form-data, so a regression to a
  # url-encoded body fails here instead of passing.
  Scenario: a file and a text field are uploaded as multipart/form-data
    Given the request form parameters are:
      | name     | value                                   |
      | metadata | {"idDocType":"ID_CARD","country":"DEU"} |
    And I attach the file "fixtures/id_front.png" to the request as "content"
    When I request "/documents" using HTTP POST
    Then the response code is 201
    And the response body contains JSON:
      """
      {"status": "stored"}
      """
