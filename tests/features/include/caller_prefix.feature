Feature: Includes the same flow twice with a call-site prefix
  Scenario: Buyer and seller registrations do not overwrite each other
    Given I include "target.feature" with prefix "buyer"
    And I include "target.feature" with prefix "seller"
    Then variable "buyer_userId" should be equal to "abc123"
    And variable "seller_userId" should be equal to "abc123"
