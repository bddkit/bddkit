Feature: Debug mode logs the prefixed export name
  Scenario: Debug mode shows the call-site prefix applied to an export
    Given I am in debug mode
    When I include "outline.feature" with prefix "buyer" with:
      | value       |
      | debug-value |
    Then variable "buyer_seen" should be equal to "debug-value"
