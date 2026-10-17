@demo @json
Feature: reading and matching JSON

  Rule: a body is matched against a description of it

    Scenario: matchers instead of literal values
      When I request "/users/1" using HTTP GET
      Then the response code is 200
      And the response body contains JSON:
        """
        {
          "id": "@variableType(int)",
          "name": "@variableType(string)",
          "email": "@regExp(/^[^@]+@example\\.com$/)",
          "address": {"geo": {"lat": "@variableType(string)"}},
          "tags": "@arrayLength(3)"
        }
        """

    Scenario: contains is a subset match, extras are allowed
      When I request "/users/1" using HTTP GET
      Then the response code is 200
      # Only the nested key is named here; every other field of the response
      # is ignored. `equals JSON` would fail on exactly this docstring.
      And the response body contains JSON:
        """
        {"address": {"city": "Gwenborough"}}
        """

    Scenario: an array matched element by element
      Given the query parameter "userId" is "1"
      When I request "/posts" using HTTP GET
      Then the response code is 200
      # Array containment is order-independent: this element may sit anywhere.
      And the response body contains JSON:
        """
        [{"id": 3, "title": "ea molestias quasi exercitationem"}]
        """

  Rule: a node is reached by its path

    Scenario: paths into an array of objects
      When I request "/posts/1/comments" using HTTP GET
      Then the response code is 200
      And the response body is a JSON array of length 2
      And the JSON node "[1].email" should exist
      And extract "[0].email" from JSON as "firstCommenter"
      And variable "firstCommenter" should be equal to "eliseo@example.com"
      # The same path syntax prints one node instead of the whole body.
      And Print response body as "[0].email"
