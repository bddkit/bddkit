@demo @eventual
Feature: an assertion that is allowed to pass later

  # /jobs/1 answers 202 "pending" twice, then 200 "done" — so the first
  # assertion below only passes on the third attempt. The mock server keeps
  # that counter for the life of its session: after a full run the job stays
  # "done", and `docker compose restart smocker` rewinds it.

  Scenario: poll a job until it finishes
    When I request "/jobs/1" using HTTP GET
    Then I expect the next assertion to pass within 5 seconds, checking every 200 milliseconds
    And the response body contains JSON:
      """
      {"status": "done"}
      """
    # The replayed request is the one that finally matched, so the code and
    # the payload can be asserted normally from here on.
    And the response code is 200
    And extract "result" from JSON as "jobResult"
    And variable "jobResult" should be equal to "42"

  # A typed parameter takes its quotes or leaves them: the type decides, so
  # `5` and `"5"` are the same step. The quoted form is what older suites wrote.
  Scenario: numbers need no quotes, and may keep them
    Given I sleep 0 seconds
    And I sleep "0" seconds
    When I request "/jobs/1" using HTTP GET
    Then I expect the next assertion to pass within "5" seconds
    And the response code is 200
