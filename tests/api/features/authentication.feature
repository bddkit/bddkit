@authentication
Feature: authentication macro

  Scenario: login exports the token
    When I login with password "correct"
    Then variable "customerJwtToken" should be equal to "tok-abc"
