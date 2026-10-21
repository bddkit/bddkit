@demo @methods
Feature: every HTTP method against the mock API

  Rule: reading never changes the resource

    Scenario: GET a collection filtered by a query parameter
      Given the "Accept" request header is "application/json"
      And the query parameter "userId" is "1"
      When I request "/posts" using HTTP GET
      Then the response code is 200
      And the response body is a JSON array of length 3
      And the response body contains JSON:
        """
        [{"id": 2, "title": "qui est esse"}]
        """

    Scenario: HEAD answers with headers only
      When I request "/posts/1" using HTTP HEAD
      Then the response code is 200
      And the "Content-Type" response header is "application/json"
      And Print response headers

    Scenario: OPTIONS advertises the allowed methods
      When I request "/posts" using HTTP OPTIONS
      Then the response code is 204
      And the "Allow" response header is "GET, POST, OPTIONS"

  Rule: writing sends a JSON body

    Background:
      Given the "Content-Type" request header is "application/json"

    Scenario: POST creates a resource and points at it
      # Marked for `--evidence marked`: this exchange is kept even on a pass.
      Given I am collecting evidence
      And the request body is:
        """
        {"title": "new post", "body": "demo", "userId": 1}
        """
      When I request "/posts" using HTTP POST
      Then the response code is 201
      And I am not collecting evidence
      And the "Location" response header is "/posts/101"
      And extract "id" from JSON as "postId"
      And variable "postId" should be equal to "101"

    Scenario: PUT replaces the whole resource
      Given the request body is:
        """
        {"title": "replaced", "body": "replaced body", "userId": 1}
        """
      When I request "/posts/1" using HTTP PUT
      Then the response code is 200
      # equals is strict: every key, nothing extra
      And the response body equals JSON:
        """
        {"id": 1, "userId": 1, "title": "replaced", "body": "replaced body"}
        """

    Scenario: PATCH changes one field
      Given the request body is:
        """
        {"title": "patched"}
        """
      When I request "/posts/1" using HTTP PATCH
      Then the response code is 200
      And the response body contains JSON:
        """
        {"title": "patched"}
        """

  Rule: removing a resource

    Scenario: DELETE answers without a body
      When I request "/posts/1" using HTTP DELETE
      Then the response code is 204

    Scenario: a missing resource is a normal, assertable answer
      When I request "/posts/999" using HTTP GET
      Then the response code is 404
      And the response body contains JSON:
        """
        {"error": "not found"}
        """

