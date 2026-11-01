Feature: SRP login

  Scenario: a scenario can register and then log in
    When I generate an SRP verifier for "user@example.test" with password "secret" as "reg"
    And the request body is:
      """
      {"identity": "user@example.test", "salt": "<<reg_salt>>", "verifier": "<<reg_verifier>>"}
      """
    And I request "/srp/register" using HTTP POST
    Then the response code is 200

    When I start an SRP login as "srp"
    And the request body is:
      """
      {"identity": "user@example.test"}
      """
    And I request "/srp/step1" using HTTP POST
    Then the response code is 200
    And extract "salt" from JSON as "serverSalt"
    And extract "b" from JSON as "serverB"

    When I complete SRP login "srp" for "user@example.test" with password "secret" salt "<<serverSalt>>" and "<<serverB>>"
    And the request body is:
      """
      {"identity": "user@example.test", "a": "<<srp_A>>", "m1": "<<srp_M1>>"}
      """
    And I request "/srp/step2" using HTTP POST
    Then the response code is 200
    And extract "m2" from JSON as "serverM2"
    And variable "serverM2" should be equal to "<<srp_M2>>"
